use super::packages::{self, Catalogue, SignedRelease};
use crate::{Error, Result};
use std::{collections::BTreeMap, path::Path, process::Stdio, time::Duration};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn command(env: &BTreeMap<String, String>) -> tokio::process::Command {
    let mut command = tokio::process::Command::new("gh");
    command
        .env_clear()
        .envs(env)
        .env_remove("GH_DEBUG")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    command
}

pub async fn tags(catalogue: &Catalogue, env: &BTreeMap<String, String>) -> Result<Vec<String>> {
    let output = tokio::time::timeout(
        Duration::from_secs(60),
        command(env)
            .args([
                "release",
                "list",
                "--repo",
                catalogue.repository,
                "--exclude-drafts",
                "--exclude-pre-releases",
                "--limit",
                "100",
                "--json",
                "tagName",
            ])
            .output(),
    )
    .await
    .map_err(|_| {
        Error::Unavailable("GitHub did not answer the extension check; try again later".into())
    })??;
    if !output.status.success() {
        return Err(github_error(&output.stderr));
    }
    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Tag {
        tag_name: String,
    }
    let tags: Vec<Tag> = serde_json::from_slice(&output.stdout)?;
    let mut versions: Vec<_> = tags
        .into_iter()
        .filter_map(|tag| {
            let version = semver::Version::parse(tag.tag_name.strip_prefix('v')?).ok()?;
            (version.pre.is_empty() && version.build.is_empty()).then_some((version, tag.tag_name))
        })
        .collect();
    versions.sort_by(|a, b| b.0.cmp(&a.0));
    Ok(versions.into_iter().map(|(_, tag)| tag).collect())
}

fn github_error(stderr: &[u8]) -> Error {
    let stderr = String::from_utf8_lossy(stderr);
    let detail = stderr
        .trim()
        .lines()
        .last()
        .unwrap_or("the download failed");
    Error::Unavailable(format!(
        "Could not download the official extension: {detail}; check your GitHub access and try again"
    ))
}

/// A single asset is streamed with a signed size limit instead of letting gh write an unbounded
/// file. Its stderr is drained independently so a diagnostic cannot block the binary stream.
pub async fn asset(
    catalogue: &Catalogue,
    env: &BTreeMap<String, String>,
    tag: &str,
    name: &str,
    path: &Path,
    limit: u64,
) -> Result<()> {
    packages::relative(name)?;
    let mut child = command(env)
        .args([
            "release",
            "download",
            tag,
            "--repo",
            catalogue.repository,
            "--pattern",
            name,
            "--output",
            "-",
        ])
        .spawn()
        .map_err(|error| {
            Error::Unavailable(format!(
                "Could not run GitHub CLI: {error}; sign in to GitHub in Chauffeur settings"
            ))
        })?;
    let mut stdout = child
        .stdout
        .take()
        .expect("gh's stdout is piped")
        .take(limit + 1);
    let mut stderr = child.stderr.take().expect("gh's stderr is piped");
    let diagnostic = tokio::spawn(async move {
        let mut collected = Vec::new();
        let mut buffer = [0; 4096];
        loop {
            let count = stderr.read(&mut buffer).await?;
            if count == 0 {
                break;
            }
            if collected.len() < 16 * 1024 {
                let kept = count.min(16 * 1024 - collected.len());
                collected.extend_from_slice(&buffer[..kept]);
            }
        }
        Ok::<_, std::io::Error>(collected)
    });
    let transfer = async {
        let mut file = tokio::fs::File::create(path).await?;
        let copied = tokio::io::copy(&mut stdout, &mut file).await?;
        if copied > limit {
            return Err(Error::BadRequest(
                "The extension download exceeds its signed size; use another release".into(),
            ));
        }
        file.flush().await?;
        file.sync_all().await?;
        let status = child.wait().await?;
        let diagnostic = diagnostic
            .await
            .map_err(|error| Error::Internal(error.into()))??;
        if !status.success() {
            return Err(github_error(&diagnostic));
        }
        Ok(())
    };
    tokio::time::timeout(Duration::from_secs(180), transfer)
        .await
        .map_err(|_| {
            Error::Unavailable(
                "The extension download timed out; check the connection and try again".into(),
            )
        })?
}

pub async fn manifest(
    catalogue: &Catalogue,
    env: &BTreeMap<String, String>,
    tag: &str,
    parent: &Path,
) -> Result<SignedRelease> {
    let directory = tempfile::tempdir_in(parent)?;
    asset(
        catalogue,
        env,
        tag,
        packages::MANIFEST,
        &directory.path().join(packages::MANIFEST),
        1024 * 1024,
    )
    .await?;
    asset(
        catalogue,
        env,
        tag,
        packages::SIGNATURE,
        &directory.path().join(packages::SIGNATURE),
        64,
    )
    .await?;
    let signed = SignedRelease::read(directory.path())?;
    let release = signed.verify(catalogue)?;
    if format!("v{}", release.version) != tag {
        return Err(Error::BadRequest(
            "The extension release tag does not match its signed version; use another release"
                .into(),
        ));
    }
    Ok(signed)
}
