# Security policy

## Reporting a vulnerability

Please **do not** open a public issue for security problems. Report them
privately through GitHub's *Security → Report a vulnerability* on this
repository, or by email to ambercode@tutamail.com. Include steps to reproduce and the
affected version or commit. We aim to acknowledge reports within 72 hours.

## What we consider in scope

- Token or credential leakage. Tokens must stay in the OS credential vault and
  must never appear in logs, the config file, crash output, the API or plugins.
- Plugin sandbox escapes: file system or process access, reading another
  plugin's storage, bypassing permission grants, or bypassing resource limits.
- Local API weaknesses: authentication bypass, CSRF or DNS rebinding from a
  browser, binding to non-loopback interfaces, or write actions while
  `allow_actions` is off.
- Remote content (messages, embeds, images) causing crashes, code execution or
  unintended network requests.

## Hardening notes

- The local API is **off by default**. It listens only on `127.0.0.1`, requires
  a random 256-bit token, and rejects foreign `Host` and `Origin` headers.
- Plugins run Luau with the standard library sandboxed. They have no `io`,
  `os.execute`, `require`, `load` or `debug`, and only the API functions for
  permissions you explicitly granted.
- The captcha helper runs out of process and only loads `discord.com` and the
  captcha provider.
