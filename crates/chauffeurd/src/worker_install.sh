# Installs Chauffeur on a GCP worker and starts its daemon as a systemd user service that outlives
# the SSH session (docs/cloud-workers.md). Run by the Mac's daemon over `gcloud compute ssh`, as the
# worker's user, after it copied the Linux bundle to ~/chauffeur-linux-x64.tar.gz. Safe to run again.
set -euo pipefail
export DEBIAN_FRONTEND=noninteractive
if ! command -v git >/dev/null || ! command -v curl >/dev/null || ! command -v gh >/dev/null || ! command -v unzip >/dev/null; then
  sudo apt-get update -qq
  sudo apt-get install -y -qq git curl ca-certificates gh unzip >/dev/null
fi
mkdir -p ~/.chauffeur/bin ~/.local/bin
tar -xzf ~/chauffeur-linux-x64.tar.gz -C ~/.chauffeur/bin
rm -f ~/chauffeur-linux-x64.tar.gz
ln -sf ~/.chauffeur/bin/chauffeur ~/.local/bin/chauffeur
# A headless VM has no Secret Service: marked a worker, the daemon keeps its secrets in
# ~/.chauffeur/secrets, readable by this user only.
echo "A GCP worker: its secrets are in secrets/." > ~/.chauffeur/worker
# No Linear (a call would move real tickets) and no Office presence from a worker.
[ -f ~/.chauffeur/settings.toml ] || printf 'github_org = ""\ngithub_sources = false\ndefault_repos = []\nlinear_enabled = false\n[office]\nurl = ""\n' > ~/.chauffeur/settings.toml
# gcloud writes the backups, as the VM's service account; Ubuntu's GCE images carry it.
command -v gcloud >/dev/null || sudo snap install google-cloud-cli --classic >/dev/null
# The AWS CLI, for the Mac's AWS SSO sign-in each send copies here (docs/cloud-workers.md).
if ! command -v aws >/dev/null; then
  rm -rf /tmp/chauffeur-awscli && mkdir /tmp/chauffeur-awscli
  curl -fsSL https://awscli.amazonaws.com/awscli-exe-linux-x86_64.zip -o /tmp/chauffeur-awscli/awscli.zip
  unzip -q /tmp/chauffeur-awscli/awscli.zip -d /tmp/chauffeur-awscli
  sudo /tmp/chauffeur-awscli/aws/install >/dev/null
  rm -rf /tmp/chauffeur-awscli
fi
# The agents' own CLIs, signed in once from the Mac (`chauffeur worker login`), their login kept here.
[ -x ~/.local/bin/claude ] || curl -fsSL https://claude.ai/install.sh | bash >/dev/null
# Codex from its release package, at the version the sidecar speaks (sidecar/src/version.ts): the
# bare binary lacks codex-code-mode-host, which runs every command of the current models, and the
# bwrap that sandboxes them.
CODEX_VERSION=0.167.0
if [ "$(cat ~/.local/lib/codex/version 2>/dev/null)" != "$CODEX_VERSION" ]; then
  rm -rf ~/.local/lib/codex && mkdir -p ~/.local/lib/codex
  curl -fsSL "https://github.com/openai/codex/releases/download/rust-v$CODEX_VERSION/codex-package-x86_64-unknown-linux-musl.tar.gz" | tar -xz -C ~/.local/lib/codex
  echo "$CODEX_VERSION" > ~/.local/lib/codex/version
fi
ln -sfn ~/.local/lib/codex/bin/codex ~/.local/bin/codex
# Ubuntu 24.04 lets only the programs AppArmor names create user namespaces; Codex's bwrap needs one.
if [ -d /etc/apparmor.d ]; then
  printf 'abi <abi/4.0>,\ninclude <tunables/global>\nprofile chauffeur-codex-bwrap %s flags=(unconfined) {\n  userns,\n}\n' \
    "$HOME/.local/lib/codex/codex-resources/bwrap" | sudo tee /etc/apparmor.d/chauffeur-codex-bwrap >/dev/null
  sudo apparmor_parser -r /etc/apparmor.d/chauffeur-codex-bwrap
fi
# Docker with compose, for the workspace's declared services (docs/workspace-development-services.md),
# from Ubuntu's own packages. The daemon reaches it as a member of the `docker` group.
if ! command -v docker >/dev/null || ! docker compose version >/dev/null 2>&1; then
  sudo apt-get update -qq
  sudo apt-get install -y -qq docker.io docker-compose-v2 >/dev/null
fi
# The toolchains a repo declares, which the daemon installs in each repo before its setup script
# (docs/cloud-workers.md): mise from its signed apt repository, reading .nvmrc, .node-version and
# .python-version too; uv for every repo; mise's shims first on the login shell's PATH, which the
# daemon reads when it starts, so that setup scripts, terminals and agents find the tools.
if ! command -v mise >/dev/null; then
  sudo install -dm 755 /etc/apt/keyrings
  curl -fsSL https://mise.jdx.dev/gpg-key.pub | sudo tee /etc/apt/keyrings/mise-archive-keyring.asc >/dev/null
  echo "deb [signed-by=/etc/apt/keyrings/mise-archive-keyring.asc arch=amd64] https://mise.jdx.dev/deb stable main" \
    | sudo tee /etc/apt/sources.list.d/mise.list >/dev/null
  sudo apt-get update -qq
  sudo apt-get install -y -qq mise >/dev/null
fi
mkdir -p ~/.config/mise/conf.d
printf '[settings]\nidiomatic_version_file_enable_tools = ["node", "python"]\n\n[tools]\nuv = "latest"\n' \
  > ~/.config/mise/conf.d/chauffeur.toml
(cd ~ && MISE_YES=1 mise install >/dev/null)
grep -q 'mise/shims' ~/.profile 2>/dev/null \
  || printf '\n# Chauffeur: the toolchains repos declare, through mise.\nexport PATH="$HOME/.local/share/mise/shims:$PATH"\n' >> ~/.profile
sudo usermod -aG docker "$(id -un)"
sudo loginctl enable-linger "$(id -un)"
# The user's systemd manager started with this SSH login, before the group: started again, its
# services, the daemon included, run with it.
sudo systemctl restart "user@$(id -u).service"
export XDG_RUNTIME_DIR="/run/user/$(id -u)"
for _ in $(seq 1 30); do [ -S "$XDG_RUNTIME_DIR/bus" ] && break; sleep 1; done
# A daemon of the previous version keeps the lock from the new one, under the service or outside
# it (started by a CLI over SSH, which the restart above does not reach): stopped first.
systemctl --user stop chauffeurd 2>/dev/null || true
pkill -TERM -u "$(id -u)" -x chauffeurd || true
for _ in $(seq 1 30); do pgrep -u "$(id -u)" -x chauffeurd >/dev/null || break; sleep 1; done
~/.chauffeur/bin/chauffeur daemon install >/dev/null
# The socket alone could be another daemon's: the one running must be the service's.
service_runs() {
  local main
  main=$(systemctl --user show -p MainPID --value chauffeurd)
  [ "$main" != 0 ] && [ -S ~/.chauffeur/chauffeurd.sock ] \
    && grep -Eq "\"pid\": *$main[^0-9]" ~/.chauffeur/daemon.json 2>/dev/null
}
for _ in $(seq 1 30); do service_runs && break; sleep 1; done
service_runs || { echo "the daemon did not start: systemctl --user status chauffeurd" >&2; exit 1; }
echo "home=$HOME"
