# Invitation-only mobile access

- **Date:** 2026-11-19
- **Status:** accepted

## Context

Dialog wants users outside its Workspace to test the mobile companion on their
own Macs. Installing the phone app and getting permission to use the hosted
relay are separate actions.

## Decision

Decided with Justin 2026-11-19: the hosted relay admits all verified Dialog
Google Workspace accounts and individually invited Google addresses. Dialog
manages that list in the relay's GCP configuration. The invitation opens only
mobile access and works for iOS and Android. Removing an address also cuts off
phones already paired under that account.

## Consequences

- The Mac reuses the signed-in Google identity. The relay verifies the signature,
  audience, expiry and e-mail ownership and authorizes room and device requests.
- Managed devices are scoped to a Google subject and their relay registration
  expires without renewal. Updating the invitation list restarts the relay, closes
  existing connections and applies the new policy before clients can reconnect.
- A distribution opts into Google access for its default relay. Self-hosted
  operator credentials continue to work. The phone pairing protocol is unchanged.
