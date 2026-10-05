# Security policy

## Supported versions

Only the latest release published on the
[Releases](https://github.com/Antxj/vpn/releases) page receives fixes. The app
checks for new versions by itself and can update with one click.

## Reporting a vulnerability

Please **do not** open a public issue for security problems. Report them
privately through GitHub instead: open the repository's **Security** tab and
click **Report a vulnerability**.

Include what you found, how to reproduce it and the app version
(shown under **Accounts**, at the bottom of the screen). You will get an answer
as soon as possible, and the fix will be credited to you in the release notes
if you wish.

## Scope

Of particular interest:

- exposure of saved accounts, passwords or TOTP seeds (stored with Windows
  DPAPI in `%APPDATA%\VPN\contas.dat`);
- the update mechanism (download, SHA-256 and signature checks, executable
  replacement);
- the OpenVPN management interface, which listens only on `127.0.0.1`;
- the routes the app creates.

Vulnerabilities in OpenVPN itself should be reported to the
[OpenVPN project](https://community.openvpn.net/openvpn/wiki/SecurityAnnouncements).

---

**Português:** não abra issue pública para problemas de segurança. Use a aba
**Security** › **Report a vulnerability** deste repositório.
