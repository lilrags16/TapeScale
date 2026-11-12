# Security policy

## Supported versions

TapeScale is currently an engineering preview. No version is qualified for production tape protection, and no production support or response-time commitment is offered yet. Security fixes are applied to the current `main` branch.

## Reporting a vulnerability

Do not open a public issue for a suspected vulnerability.

Use [GitHub private vulnerability reporting](https://github.com/lilrags16/TapeScale/security/advisories/new) to contact the maintainers privately. Include, when available:

- the affected commit or version;
- the component and deployment configuration;
- reproduction steps or a minimal proof of concept;
- likely impact and required privileges;
- whether credentials, recovery material, media, or destructive operations are involved;
- suggested mitigations, without including real secrets or customer data.

You should receive an acknowledgement through the private advisory. Please allow time for validation before public disclosure. Maintainers will coordinate remediation and attribution through that advisory.

## Safe research

Use only systems, datasets, and media you own or are explicitly authorized to test. Do not test destructive media, device, restore, or path-handling behavior against production equipment or irreplaceable data.

The product security model and production release blockers are documented in [docs/SECURITY.md](docs/SECURITY.md).
