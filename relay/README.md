# Chauffeur relay and local mobile host

Opaque WebSocket forwarding with authenticated, bounded, one-use QR rooms and
renewable device registrations. The relay
has no agent API and no endpoint identity keys. See the [shared transport](../packages/remote/README.md)
for the pairing protocol and its current limits.

## Mobile dictation on GCP

`POST /v1/devices/{id}/voice-session` exchanges an approved phone's bearer
credential for a model-scoped Mistral realtime token. Both the registered phone
and its Mac must be connected. The endpoint receives no audio or transcript;
the phone streams directly to Mistral. It returns `503` when voice is not configured.
Issuance is limited per device and globally, with a ten-second provider timeout.

Keep the dedicated Mistral key in Secret Manager as `chauffeur-mistral-api-key`.
The relay service account gets access to that secret only. Do not put it in the
app, EAS variables, repository, command arguments or logs. Add its value through
Secret Manager's **New version** UI, then use `deploy/configure-voice.sh
--existing-secret` with explicit `GCP_PROJECT`, `GCP_REGION`, `GCP_ZONE` and
`GCP_INSTANCE`. This configures access and metadata; it does not restart anything.

For the existing Paris deployment, `deploy/rollout-voice.sh` packages the local
reviewed relay, uploads the private artifact, and swaps only the relay service
over IAP. It preserves the existing environment (including APNS), fetches the
Mistral key on the VM, verifies the archive checksum, checks local health, and
rolls back on failure. It also updates reboot metadata. Set
`RELAY_ARTIFACTS_BUCKET` and `EXPECTED_RELAY_SHA256` from the current reviewed VM
metadata along with the explicit project, zone and instance. Do not run a fresh
provisioning script or rerun the full VM startup script for this change.

Mobile dictation additionally requires a new native mobile build and physical
iPhone/Android validation. A healthy relay alone does not validate microphone
permissions, PCM audio, or Mistral transcription end to end.

## Local verification

```sh
cd ../packages/remote
bun install --frozen-lockfile
cd ../../relay
bun install --frozen-lockfile
bun run typecheck
bun test
```

To run the proof manually, generate an operator token in your terminal and export
it in both local shells. Do not put it in a command argument, git or a shared log:

```sh
export RELAY_ADMIN_TOKEN=$(openssl rand -hex 32)
bun start
```

In a second shell with that same token, run `bun run pair`. It displays a one-use
QR. Scan it from the mobile Connection setup screen. The `--json` option prints
the same invitation for local paste testing; treat that output like the QR.
The default relay is `ws://127.0.0.1:41240`, intended only for loopback tests.
For an Android device attached by USB, `adb reverse tcp:41240 tcp:41240` can map
that loopback address to the Mac. A phone on Wi-Fi needs the HTTPS/WSS deployment.
There is deliberately no insecure LAN-URL fallback.

Wireless ADB is also supported: associate the Pixel using Android's Wireless
debugging pairing dialog, then use `adb reverse` on that Wi-Fi transport instead
of USB. `bun run test:phone` starts a disposable relay on 41240 and a Mac-only
pairing page at `http://127.0.0.1:41241`. It keeps all test credentials in memory
and prints no invitation to logs. Generate the QR only when the app is ready to
scan it. Also reverse Metro's port (41242 for this workspace). See the mobile
companion's instructions (Pixel over Wi-Fi) in its own repository.

## Durable local pairing

`bun run mobile:host` starts the local relay and pairing page with the private
daemon bridge. It requires the new daemon's `remote.*` methods. The daemon owns
all persisted Mac identity and device credentials; this process only reads them
over the private Unix socket. Scan one QR, compare the phone identity and approve
on the Mac. The encrypted reply supplies a reconnect profile to SecureStore.
The app then connects to the remembered device channel without another scan.

`remote.state` contains secrets and is host-only; the mobile RPC allowlist never
forwards it or any other `remote.*` method. Public device info on the page excludes
secrets. Revoke access removes the grant in the daemon, closes the session and
deletes the relay registration. A check before every mobile RPC prevents revoked
devices from issuing further commands, even before the one-second host refresh.

Workspace transfers are also forwarded, the one workspace mutation: their status, the
declared machines' names, and start or cancel between this Mac and a machine named in its
Settings → Transfers. Sockets and a named controller, which the daemon accepts on this
Unix endpoint from the CLI, are refused here; a workspace subscription only passes the
workspace's `transfer` events ([transfer](../docs/cloud-workspace-transfer.md#on-the-phone)).

A moved workspace's declared previews are listed (`previews.list`, the workspace only) and
opened through tunnels of the session: each is one connection the phone's web view opened,
passed to the daemon's `/preview/{workspace}/{offset}` on this Unix endpoint once the device
is checked, which refuses a port not declared as a preview on the machine it runs on
([previews on the phone](../docs/workspace-relay.md#previews-on-the-phone)).

Device registrations are in-memory relay metadata (id and role tokens), leased
for 24 hours and renewed every ten minutes. They contain no peer private keys,
reconnect secret or conversation data. The host re-registers after a relay restart;
both endpoints perform a fresh, mutually pinned Noise handshake on reconnect.
An invalid peer or replay cannot inherit authorization from an earlier session.

The manual development host remains a separate process and must be running.
Packaged desktop builds supervise their own host after activation in Settings >
Mobile; the manual commands below do not install a login service. GCP deployment still packages only `src/main.ts`
and the opaque server, never the private daemon bridge or its state. The local tests use ADB Wi-Fi forwarding. Cloud deployment is validated separately
below; a saved local phone profile does not automatically switch relay.

Tests include a simulated phone lifecycle with real WebSockets/Noise:

```sh
bun test
CHAUFFEUR_TEST_DAEMON="$PWD/../target/debug/chauffeurd" bun test test/daemon.integration.test.ts
```

The real-daemon test creates an isolated state directory with Linear disabled;
it checks identity/device persistence across restart and revocation without
starting an agent. `--diagnose` on the local host logs frame lengths/directions and
closure codes only; no payload, URL credential or key is logged.

## GCP deployment preparation

`deploy/gcp.sh` provisions a single `e2-small` Debian VM in one zone (`<zone>`), a
10 GiB balanced boot disk, static IPv4, private artifact bucket and a Secret
Manager operator token with user-managed replication restricted to that zone's region. A dedicated service account can read only that secret
and the artifact bucket. The public firewall permits HTTP/HTTPS; SSH is reachable
only through IAP. The relay listens on loopback behind Caddy's automatic TLS.

The relay runs in a read-only, unprivileged Bun container with resource limits.
No conversation data or private device keys are uploaded in its source archive.
The artifact checksum is verified before startup. Service logs contain startup
messages only; Caddy access logging is not enabled.

Prerequisites: a GCP project of yours with billing enabled, and suitable IAM access. The
script reads it from `GCP_PROJECT` and `GCP_REGION` (`GCP_ZONE` defaults to `<region>-a`;
`RELAY_ARTIFACTS_BUCKET` and `RELAY_ADMIN_SECRET` default to `<project>-relay-artifacts` and
`chauffeur-relay-admin`). Deploy directly on the reserved public IPv4, without DNS:

```sh
GCP_PROJECT=<project> GCP_REGION=<region> bash deploy/gcp.sh --ip
```

IP mode uses Certbot 5.4.0 and Let's Encrypt's six-day `shortlived` certificates.
Caddy serves HTTP-01 challenges on port 80. An hourly systemd timer checks for
renewal; its deploy hook installs restricted certificate copies and reloads
Caddy. Port 443 uses a publicly trusted certificate for the IP. Verify the timer
and a renewal dry run on the VM. Keep the static IP reserved.

Alternatively, with a DNS hostname you control:

```sh
bash deploy/gcp.sh relay.your-domain.example
```

This is an explicit, billable deployment command. It is not invoked by builds or
tests. Expect roughly $20–30/month for a lightly used single VM, disk, IPv4 and
traffic, varying with region/pricing. Set billing alerts in GCP; they are not a
spending cap. The script outputs the endpoint (and DNS record in hostname mode), then check `/health`
over HTTPS. A repeated invocation does not replace an existing VM.

The script's shell syntax is checked locally; actual GCP provisioning/startup and
certificate issuance must be verified after deployment. Connection tests never
provision cloud resources.

For a remote connection proof, retrieve the operator credential only in your own
terminal, set `RELAY_ORIGIN=wss://your-relay-hostname`, then run `bun run pair`.
This command remains a disposable proof. Agent access uses the private Mac host
and daemon approval described above. Only QR rooms expire after five minutes.

This is a single-instance prototype: a relay restart ends open connections. QR
rooms must be generated again; approved device profiles reconnect after the host
re-registers. Agents keep running in the daemon while the relay is unavailable.

To stop ongoing VM charges, delete the relay VM in `<zone>` and its static
address in `<region>`. The private bucket and secret
remain separately billable until removed. No cleanup command is run automatically.

The durable Mac host accepts `RELAY_ORIGIN=wss://<IP-or-hostname>` and
`RELAY_ADMIN_TOKEN` in its environment. With these set, `bun run mobile:host`
connects to the cloud relay instead of starting one on port 41240. The pairing
page stays private on `127.0.0.1:41241`. Existing local profiles keep their relay;
pair once with the new QR to save the cloud endpoint. Keep the operator token
out of command arguments, git and logs. No Mac bridge or daemon state is uploaded.

## Operations and logs

Deployment uses one zone (`<zone>`), a private artifact bucket and an operator-token
secret replicated only in that zone's region (`<region>`). IAM and the VPC
are global control-plane resources. Agent state never leaves the Mac.

Logs stay in the VM's persistent systemd journal, bounded to 100 MiB and seven
days. Docker's duplicate JSON-file logging is disabled. There is no Cloud Logging
agent or HTTP access log. Relay lifecycle events are structured JSON: local trace
UUID, host/phone role, closure code, duration, aggregate forwarded byte count and
fixed failure reasons. Traces are random per connection and cannot be used for
relay authentication. There is one health summary per minute and a 200-events per
minute cap, with a suppression count. Never add request URLs/headers, room/device
IDs, peer keys, tokens, QR codes, messages or ciphertext to production logs.

Connect through IAP (no public SSH):

```sh
gcloud compute ssh <relay-vm> --project=<project> \
  --zone=<zone> --tunnel-through-iap
sudo journalctl -u chauffeur-relay -u caddy --since '30 minutes ago'
sudo journalctl -u chauffeur-relay-certificate --since '7 days ago'
systemctl status chauffeur-relay caddy chauffeur-relay-certificate.timer
systemctl list-timers chauffeur-relay-certificate.timer
sudo /opt/certbot/bin/certbot renew --dry-run
```

The relay cannot diagnose the contents of an E2E handshake or an agent RPC.
Correlate its connection times/codes with the Mac host and mobile errors. A cloud
probe uses temporary identities and synthetic data only:

```sh
# Supply RELAY_ORIGIN and RELAY_ADMIN_TOKEN privately in the environment.
bun deploy/smoke.ts
```

Freeze a rollout's deploy scripts and the two relay sources in a scratch folder
before executing it. Never edit a shell script while that invocation is running.
The existing-instance guard does not roll out later source changes automatically.

An IP-mode deployment was verified on 2026-10-18: `https://<ip>/health` returned
`ok` with normal certificate validation. A synthetic cloud probe passed pinned Noise pairing
and encrypted delivery; the certificate renewal dry run succeeded. IAP SSH and
structured service journals were checked on the live VM. Caddy's IP mode sets
`default_sni` to the public IP so non-SNI clients receive the correct certificate
behind Compute Engine NAT. This is included in startup metadata for reboot.
No cloud agents or workspace migration is enabled by the relay.

Device sessions expire a peer after 20 seconds without incoming application
traffic, even if its counterpart continues sending data. This releases a phone
slot left half-open by Wi-Fi/cellular handover instead of waiting minutes for TCP
to time out. Current clients ping every five seconds with one five-second pong
deadline. The pinned Noise handshake and saved trust are required on every retry;
no user command is automatically replayed. A regression test models a silent
mobile socket while its counterpart stays active.

## Integrated desktop host

The packaged app includes `chauffeur-mobile-host`, built with `bun run build:host`.
Enable it in Settings > Mobile with the WSS relay origin and its operator
credential. Chauffeur's daemon stores that credential in the Mac Keychain (the
isolated daemon's secrets folder during tests), passes it to its child on stdin,
and supervises the bridge. No Bun or Google Cloud CLI is required on the installed
machine. Closing the Mac window leaves remote access available; disabling stops
it without revoking devices. The relay configuration must be set once; an old
workspace launchd helper must be stopped before enabling the integrated host,
because both would claim the same pairing-page port and device slots.

The local health endpoint is authenticated with a fresh per-launch control token;
status RPCs expose only configuration/readiness/counts, never credentials. No
private host configuration methods are allowed through the mobile bridge.

```sh
bun run build:host
CHAUFFEUR_TEST_DAEMON="$PWD/../target/debug/chauffeurd" \
CHAUFFEUR_TEST_MOBILE_HOST="$PWD/dist/chauffeur-mobile-host" bun test
```
