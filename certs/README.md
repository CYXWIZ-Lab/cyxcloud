# CyxCloud TLS Certificates

This directory contains TLS certificates for CyxCloud mTLS (mutual TLS) authentication.

## Development Certificates

Generate development certificates using the provided script:

```bash
# From the project root
./scripts/generate-dev-certs.sh

# Or specify a custom output directory
./scripts/generate-dev-certs.sh /path/to/certs
```

### Generated Files

| File | Purpose |
|------|---------|
| `ca.crt` | Root CA certificate (trust anchor) |
| `ca.key` | Root CA private key (keep secure!) |
| `gateway.crt` | Gateway server certificate |
| `gateway.key` | Gateway server private key |
| `node1.crt/key` | Storage node 1 certificate/key |
| `node2.crt/key` | Storage node 2 certificate/key |
| `node3.crt/key` | Storage node 3 certificate/key |
| `cli.crt/key` | CLI client certificate/key |

## Production Certificates

For production deployments:

1. **Use a real Certificate Authority** (Let's Encrypt, DigiCert, etc.)
2. **Never use self-signed certificates** in production
3. **Rotate certificates regularly** (recommended: every 90 days)
4. **Protect private keys** with proper file permissions (600)
5. **Use hardware security modules (HSM)** for CA keys if possible

### Production Certificate Requirements

**Gateway Certificate:**
- Subject Alternative Names (SANs) must include all gateway hostnames
- Extended Key Usage: `serverAuth, clientAuth`

**Node Certificates:**
- Common Name should match node ID
- Extended Key Usage: `serverAuth, clientAuth`

**CLI Certificates:**
- Extended Key Usage: `clientAuth`
- Issued per-user for audit trails

## Environment Variables

| Variable | Description |
|----------|-------------|
| `TLS_CERT` | Path to server certificate |
| `TLS_KEY` | Path to server private key |
| `TLS_CA_CERT` | Path to CA certificate for verification |
| `TLS_CLIENT_CERT` | Path to client certificate (mTLS) |
| `TLS_CLIENT_KEY` | Path to client private key (mTLS) |

## Docker Usage

Mount certificates into containers:

```yaml
services:
  gateway:
    volumes:
      - ./certs:/certs:ro
    environment:
      TLS_CERT: /certs/gateway.crt
      TLS_KEY: /certs/gateway.key
      TLS_CA_CERT: /certs/ca.crt
```

## Security Notes

- **Never commit certificates to git** (they're in .gitignore)
- **Never share private keys** between services
- **Use separate certificates** for each node
- **Monitor certificate expiration** and rotate before expiry
