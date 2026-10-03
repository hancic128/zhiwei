# Security Policy

## Supported Versions

| Version | Supported          |
| ------- | ------------------ |
| 0.0.x   | :white_check_mark: |

## Reporting a Vulnerability

We take security vulnerabilities seriously. If you discover a security issue,
please report it responsibly.

### How to Report

**Please DO NOT file a public GitHub issue** for security vulnerabilities.
Instead, please email us directly:

- **Email**: security@zhiwei.example.invalid

### What to Include

Your report should include:

1. **Description**: A clear description of the vulnerability
2. **Steps to Reproduce**: How to reproduce the issue (if applicable)
3. **Impact**: Potential impact of the vulnerability
4. **Affected Version**: Which versions are affected
5. **Suggested Fix** (optional): How you think it could be fixed

### Response Timeline

| Phase | Timeline |
| --- | --- |
| Initial Response | Within 48 hours |
| Assessment | Within 7 days |
| Fix Development | Depends on severity |
| Disclosure | After fix is available |

### Severity Classification

| Severity | Definition | Response |
| --- | --- | --- |
| Critical | Remote code execution, complete system compromise | Immediate attention |
| High | Significant security impact | Within 7 days |
| Medium | Moderate security impact | Within 30 days |
| Low | Minimal security impact | Best effort |

### Security Updates

Security fixes will be released as patch versions (e.g., v0.1.1) and
announced through:

- GitHub Security Advisories
- Release notes (without details until widely deployed)

### Scope

This security policy applies to:

- `zhiwei-monitor`: The server component
- `zhiwei-node`: The node agent
- `zhiwei-ops`: The operations server

Third-party dependencies are covered by their respective security policies.

## Security Best Practices

When deploying ZhiWei, we recommend:

1. **Keep bootstrap tokens secure**: They expire after 10 minutes by default
2. **Protect signing keys**: Node Ed25519 signing keys should have 0600 permissions
3. **Use HTTPS in production**: Either self-signed certificates or behind a reverse proxy
4. **Restrict network access**: Only allow necessary ports through firewall
5. **Monitor logs**: Regularly review logs for authentication failures

## Security Model

See [README.md](./README.md) for details on:
- Ed25519 request signing vs mTLS
- Bidirectional command signing
- Time window and nonce replay protection
