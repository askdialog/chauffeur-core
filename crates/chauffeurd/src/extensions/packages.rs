//! Verified immutable engine/UI packages. The daemon is the only writer; activation and the
//! installed-version journal are separate from downloading and expanding an archive.
use crate::{Error, Result};
use chauffeur_protocol::extensions::{ExtensionId, ExtensionRelease, ExtensionReleaseTarget};
use ed25519_dalek::{Signature, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    io::Read,
    path::{Component, Path, PathBuf},
};

pub const MANIFEST: &str = "extension.json";
pub const SIGNATURE: &str = "extension.json.sig";
pub const MAX_ARCHIVE: u64 = 256 * 1024 * 1024;
const MAX_EXPANDED: u64 = 512 * 1024 * 1024;
const MAX_ENTRIES: usize = 20_000;

#[derive(Clone)]
pub struct Catalogue {
    pub id: ExtensionId,
    pub repository: &'static str,
    pub public_key: [u8; 32],
}

pub fn slug(id: ExtensionId) -> &'static str {
    match id {
        ExtensionId::Database => "database",
        ExtensionId::Office => "office",
    }
}

pub fn platform() -> Result<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => Ok("aarch64-apple-darwin"),
        ("macos", "x86_64") => Ok("x86_64-apple-darwin"),
        ("linux", "x86_64") => Ok("x86_64-unknown-linux-gnu"),
        _ => Err(Error::Unavailable("Official extension packages are unavailable for this platform; use a supported Chauffeur build".into())),
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SignedRelease {
    pub json: String,
    pub signature: Vec<u8>,
}

impl SignedRelease {
    pub fn read(dir: &Path) -> Result<Self> {
        let json = bounded_read(&dir.join(MANIFEST), 1024 * 1024)?;
        let signature = bounded_read(&dir.join(SIGNATURE), 64)?;
        Ok(Self {
            json: String::from_utf8(json).map_err(|error| Error::Internal(error.into()))?,
            signature,
        })
    }

    pub fn verify(&self, catalogue: &Catalogue) -> Result<ExtensionRelease> {
        let invalid = || {
            Error::BadRequest("This extension package has an invalid signature; download a release published by Chauffeur".into())
        };
        let key = VerifyingKey::from_bytes(&catalogue.public_key).map_err(|_| invalid())?;
        let bytes: [u8; 64] = self
            .signature
            .as_slice()
            .try_into()
            .map_err(|_| invalid())?;
        key.verify_strict(self.json.as_bytes(), &Signature::from_bytes(&bytes))
            .map_err(|_| invalid())?;
        let release: ExtensionRelease = serde_json::from_str(&self.json)?;
        if release.id != catalogue.id || release.manifest_version != 1 || release.state_format != 1
        {
            return Err(Error::BadRequest("This package belongs to another extension or needs a newer package format; update Chauffeur".into()));
        }
        let version = semver::Version::parse(&release.version).map_err(|_| {
            Error::BadRequest(
                "The extension version is invalid; use a stable published release".into(),
            )
        })?;
        if !version.pre.is_empty() || !version.build.is_empty() {
            return Err(Error::BadRequest(
                "This extension is not a stable release; use a published stable version".into(),
            ));
        }
        Ok(release)
    }
}

pub fn compatible(release: &ExtensionRelease, core: &str) -> Result<()> {
    let requirement = semver::VersionReq::parse(&release.requires_core).map_err(|_| {
        Error::BadRequest(
            "The extension declares an invalid Core range; use another published version".into(),
        )
    })?;
    let core = semver::Version::parse(core).map_err(|error| Error::Internal(error.into()))?;
    if !requirement.matches(&core)
        || release.host_api != chauffeur_extension_api::HOST_API_VERSION
        || release.ui_api != chauffeur_extension_api::UI_API_VERSION
    {
        return Err(Error::Unavailable(format!(
            "{} {} needs another Chauffeur version ({}); update Chauffeur before installing it",
            slug(release.id),
            release.version,
            release.requires_core
        )));
    }
    Ok(())
}

pub fn target(release: &ExtensionRelease) -> Result<ExtensionReleaseTarget> {
    let platform = platform()?;
    let targets: Vec<_> = release
        .targets
        .iter()
        .filter(|target| target.platform == platform)
        .collect();
    if targets.len() != 1 {
        return Err(Error::Unavailable(format!(
            "{} {} has no unique package for {platform}; use another release",
            slug(release.id),
            release.version
        )));
    }
    let target = targets[0];
    if target.bytes == 0
        || u64::from(target.bytes) > MAX_ARCHIVE
        || target.sha256.len() != 64
        || !target
            .sha256
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err(Error::BadRequest(
            "The extension archive metadata is invalid; use another release".into(),
        ));
    }
    let archive = relative(&target.archive)?;
    if archive.components().count() != 1 {
        return Err(Error::BadRequest(
            "The extension archive must be a single file name; use another release".into(),
        ));
    }
    relative(&target.executable)?;
    relative(&target.script)?;
    relative(&target.assets)?;
    if let Some(stylesheet) = &target.stylesheet {
        relative(stylesheet)?;
    }
    Ok(target.clone())
}

pub fn relative(value: &str) -> Result<PathBuf> {
    let path = Path::new(value);
    if value.is_empty()
        || value.contains('\\')
        || !path
            .components()
            .all(|part| matches!(part, Component::Normal(_)))
    {
        return Err(Error::BadRequest(
            "The extension contains a non-relative path; use another release".into(),
        ));
    }
    Ok(path.to_path_buf())
}

fn bounded_read(path: &Path, max: u64) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    std::fs::File::open(path)?
        .take(max + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > max {
        return Err(Error::BadRequest(
            "An extension file exceeds its allowed size; use another release".into(),
        ));
    }
    Ok(bytes)
}

pub fn digest(path: &Path) -> Result<String> {
    let mut file = std::fs::File::open(path)?;
    let mut digest = Sha256::new();
    let mut buffer = [0; 64 * 1024];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
    }
    Ok(hex::encode(digest.finalize()))
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Installed {
    pub release: SignedRelease,
    pub files: BTreeMap<String, String>,
}

impl Installed {
    pub fn metadata(
        &self,
        catalogue: &Catalogue,
    ) -> Result<(ExtensionRelease, ExtensionReleaseTarget)> {
        let release = self.release.verify(catalogue)?;
        compatible(&release, env!("CARGO_PKG_VERSION"))?;
        let target = target(&release)?;
        Ok((release, target))
    }
    pub fn root(&self, catalogue: &Catalogue, packages: &Path) -> Result<PathBuf> {
        let (_, target) = self.metadata(catalogue)?;
        Ok(packages.join(target.sha256))
    }
    pub fn check_files(&self, catalogue: &Catalogue, packages: &Path) -> Result<PathBuf> {
        let root = self.root(catalogue, packages)?;
        if std::fs::symlink_metadata(&root)?.file_type().is_symlink() {
            return Err(Error::Unavailable(
                "The installed extension folder changed; reinstall it".into(),
            ));
        }
        let canonical = root.canonicalize()?;
        for (name, expected) in &self.files {
            let path = root.join(relative(name)?);
            if !std::fs::symlink_metadata(&path)?.file_type().is_file()
                || !path.canonicalize()?.starts_with(&canonical)
                || digest(&path)? != *expected
            {
                return Err(Error::Unavailable(
                    "An installed extension file changed; reinstall the extension".into(),
                ));
            }
        }
        let (_, target) = self.metadata(catalogue)?;
        for name in [&target.executable, &target.script]
            .into_iter()
            .chain(target.stylesheet.iter())
        {
            if !self.files.contains_key(name) {
                return Err(Error::BadRequest(
                    "The extension entry point is missing; reinstall the extension".into(),
                ));
            }
        }
        Ok(root)
    }
}

pub fn expand(
    release: SignedRelease,
    catalogue: &Catalogue,
    archive: &Path,
    packages: &Path,
) -> Result<Installed> {
    use std::os::unix::fs::PermissionsExt;
    let metadata = release.verify(catalogue)?;
    compatible(&metadata, env!("CARGO_PKG_VERSION"))?;
    let target = target(&metadata)?;
    if std::fs::metadata(archive)?.len() != u64::from(target.bytes)
        || digest(archive)? != target.sha256
    {
        return Err(Error::BadRequest(
            "The downloaded extension does not match its signed checksum; download it again".into(),
        ));
    }
    std::fs::create_dir_all(packages)?;
    crate::paths::private_dir(packages)?;
    let stage = tempfile::Builder::new()
        .prefix("staging-")
        .tempdir_in(packages)?;
    crate::paths::private_dir(stage.path())?;
    let gzip = flate2::read::GzDecoder::new(std::fs::File::open(archive)?);
    let mut tar = tar::Archive::new(gzip);
    let mut total = 0_u64;
    let mut files = BTreeMap::new();
    for (index, entry) in tar.entries()?.enumerate() {
        if index >= MAX_ENTRIES {
            return Err(Error::BadRequest(
                "The extension archive contains too many files; use another release".into(),
            ));
        }
        let mut entry = entry?;
        let name = entry
            .path()?
            .to_str()
            .ok_or_else(|| {
                Error::BadRequest("An extension path is not UTF-8; use another release".into())
            })?
            .to_string();
        let path = stage.path().join(relative(&name)?);
        let kind = entry.header().entry_type();
        if kind.is_dir() {
            std::fs::create_dir_all(&path)?;
            continue;
        }
        if !kind.is_file() {
            return Err(Error::BadRequest(
                "The extension archive contains a link or special file; use another release".into(),
            ));
        }
        total = total
            .checked_add(entry.size())
            .filter(|total| *total <= MAX_EXPANDED)
            .ok_or_else(|| {
                Error::BadRequest("The expanded extension is too large; use another release".into())
            })?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)?;
        std::io::copy(&mut entry, &mut file)?;
        file.sync_all()?;
        std::fs::set_permissions(
            &path,
            std::fs::Permissions::from_mode(if name == target.executable {
                0o700
            } else {
                0o600
            }),
        )?;
        files.insert(name, digest(&path)?);
    }
    for name in [&target.executable, &target.script]
        .into_iter()
        .chain(target.stylesheet.iter())
    {
        if !files.contains_key(name) {
            return Err(Error::BadRequest(
                "The extension archive is missing an entry point; use another release".into(),
            ));
        }
    }
    let installed = Installed { release, files };
    let destination = packages.join(&target.sha256);
    if std::fs::symlink_metadata(&destination).is_ok() {
        if installed.check_files(catalogue, packages).is_ok() {
            return Ok(installed);
        }
        // A damaged copy of this revision: the verified one replaces it, or reinstalling could
        // never repair it. A running engine keeps its open files.
        let damaged = tempfile::Builder::new()
            .prefix("damaged-")
            .tempdir_in(packages)?;
        std::fs::rename(&destination, damaged.path().join("package"))?;
    }
    std::fs::rename(stage.path(), &destination)?;
    Ok(installed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};
    #[test]
    fn a_signature_cannot_be_reused_for_modified_metadata_or_another_extension() {
        let key = SigningKey::from_bytes(&[7; 32]);
        let catalogue = Catalogue {
            id: ExtensionId::Database,
            repository: "fixture",
            public_key: key.verifying_key().to_bytes(),
        };
        let json = serde_json::to_string(&ExtensionRelease {
            manifest_version: 1,
            id: ExtensionId::Database,
            version: "1.0.0".into(),
            requires_core: ">=0.30.0".into(),
            host_api: 1,
            ui_api: 1,
            state_format: 1,
            targets: vec![],
        })
        .unwrap();
        let mut release = SignedRelease {
            signature: key.sign(json.as_bytes()).to_bytes().to_vec(),
            json,
        };
        assert!(release.verify(&catalogue).is_ok());
        release.json = release.json.replace("1.0.0", "1.0.1");
        assert!(release.verify(&catalogue).is_err());
    }
    #[test]
    fn packages_cannot_name_absolute_parent_or_windows_paths() {
        for path in [
            "/etc/passwd",
            "../state.sqlite",
            "ui/../../state.sqlite",
            "ui\\entry.js",
            "",
        ] {
            assert!(relative(path).is_err(), "{path}");
        }
        assert!(relative("ui/assets/office/villager.glb").is_ok());
    }
}
