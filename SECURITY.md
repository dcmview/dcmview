# Security Policy

## Supported Versions

Security fixes are handled for the current released version of `dcmview`. If a
security issue affects older versions, maintainers will document the affected
range in the release notes when a fix is available.

## Reporting a Vulnerability

Please disclose suspected security vulnerabilities to the maintainers privately
before public disclosure. Do not include vulnerability details, proof-of-concept
payloads, DICOM files, PHI, credentials, or other sensitive information in public
GitHub issues.

If GitHub private vulnerability reporting is available for this repository, use
that channel. Otherwise, contact the maintainers directly and include:

- A short description of the issue.
- The affected `dcmview` version and install channel.
- Reproduction steps that do not include PHI or sensitive data.
- Any relevant logs with paths, patient identifiers, and hostnames redacted.

Maintainers will acknowledge reports, assess impact, and coordinate a fix before
public disclosure when the report describes a real vulnerability.

## Security Model

`dcmview` is intended for research and development inspection on secure
networks. It is not for clinical diagnosis or clinical decision-making.

Every `/api` request requires the session's bearer token by default, including
health, downloads, and unknown routes. The viewer page and hashed assets are
public and contain no file data. A generated token uses 32 bytes of OS
randomness as unpadded base64url and is compared in constant time. It is valid
for the process lifetime, with no expiry or rotation. `DCMVIEW_TOKEN` can fix
its value; it is never accepted as a command-line argument. Treat startup
JSON and the printed launch URL, whose fragment carries the token, as
credentials.

A fixed `DCMVIEW_TOKEN` is only as strong as the value chosen: there is no
rate limit, so use a long random value. When dcmview opens a browser itself it
passes the launch URL, token included, to the operating system's opener, and
on some systems other local users can read another process's arguments. On a
machine shared with people who should not see the data, start with
`--no-browser` and open the printed link yourself, or use `--unix-socket`.

The server binds to `127.0.0.1` by default and should normally be accessed
locally or through SSH port forwarding. Bearer authentication does not encrypt
plain HTTP traffic. Avoid public-facing binds such as `--host 0.0.0.0` unless
you provide your own network access controls. Unix sockets also require the
token and restrict direct connections to the server's user ID.

`--no-token` explicitly disables API authentication and warns on stderr; it is
intended for a listener behind a proxy that already authenticates. In that
mode, anything that can reach the listener can use the API.

DICOM files often contain protected or sensitive information. Anyone with the
token and access to the listener can access image pixels, metadata, file paths,
patient identifiers, study identifiers, and in-memory annotations.
