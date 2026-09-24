# Couch UniFi Protect integration

This repository builds the independently installable UniFi Protect camera
package for Couch. It is read-only: it lists cameras, returns bounded JPEG
snapshots, and forwards the existing low-quality RTSPS H264 stream to Couch's
decoder. It never creates, enables, disables, or deletes Protect streams.

## Setup

The connection form asks for only:

- the NVR's LAN IP address; and
- a UniFi Protect Integration API key.

On **Connect**, the package observes the NVR's API and media certificates
without sending the key, stores their SHA-256 fingerprints in Couch's private
credential file, then validates the key over the pinned API connection. Every
later connection requires those exact certificates. If the NVR certificate is
replaced, connect it again and review the address before accepting the new
fingerprints.

The camera's **low-quality RTSPS stream** must already be enabled in Protect.
The package deliberately does not mutate this shared NVR setting. Live views
are H264 over RTSPS/TCP and end after at most 60 seconds.

## Development

```sh
cargo test --locked --all-targets
cargo build --locked --release --target armv7-unknown-linux-musleabihf \
  --bin couch-plugin-unifi-protect
```

`integration.json` is the Couch feed descriptor and `plugin.json` is the
runtime manifest. Both declare package protocol 4. The Couch SDK dependencies
are pinned to one immutable full commit; update the two pins and `Cargo.lock`
together.

The test suite uses loopback TLS and RTSP peers. It requires no NVR, camera,
certificate, API key, or internet service after dependencies are cached.

## Security

API keys, stream URLs, SDES material, and session identifiers are never logged.
The API key is a secret setting stored by Couch under the connection's private
directory. Certificate fingerprints are an opaque Couch credential and are
not exposed through the settings API. The media side channel carries only
bounded Annex-B H264 records—never a provider URL or credential.
