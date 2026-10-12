# Kovion Loader

Kovion's own iOS sideloader. Signs and installs an `.ipa` onto a connected iPhone using your
own free Apple ID — the same technique AltServer, SideStore and iloader use — so Kovion iTools
never has to open a separate app to install itself or send an update.

Built directly on [isideload](https://github.com/nab138/isideload) (MIT, by nab138), the same
library [iloader](https://github.com/nab138/iloader) uses. This project only adds the small
amount of glue code to drive it from a simple stdin/stdout protocol, so Kovion iTools can show
its own Apple ID sign-in screen instead of a third-party one.

## Protocol

One JSON line on stdin:
```json
{"apple_id": "you@example.com", "password": "...", "ipa_path": "C:\path\Kovion.ipa"}
```

Then newline-delimited JSON events on stdout:
- `{"event":"status","message":"..."}`
- `{"event":"need_2fa","unknown":false,"sms":false,"numbers":[...],"lastError":null}` — reply
  with one line on stdin: `{"SubmitCode":"123456"}`, `{"SendSms":<id>}`, `"SendToDevices"`,
  `"ResendCode"` or `"Abort"`.
- `{"event":"progress","fraction":0.0-1.0}`
- `{"event":"done","ok":true}` or `{"event":"error","message":"..."}` (exits non-zero)

Team selection and the free-account certificate limit are both handled automatically (first
team, oldest certificate revoked if needed) — nothing else blocks.

A successful sign-in's certificates are cached in Windows Credential Manager under the service
name passed as `service` (defaults to `Kovion`), the same way SideStore/iloader cache theirs, so
re-signing later doesn't always need the password again.

## Building

```
cargo build --release
```

Windows only for now (uses the `windows-native` keyring backend). GitHub Actions builds it on
every push and publishes `kovion-sideload.exe` on the `latest` release.
