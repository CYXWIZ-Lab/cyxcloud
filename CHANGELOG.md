# Changelog

All notable changes to CyxCloud will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.2.0] - 2025-12-31

### Added

#### Mutual TLS (mTLS) Support
Complete end-to-end TLS encryption for all CyxCloud components:

- **Gateway HTTPS/gRPC TLS**
  - HTTPS server with TLS 1.3 support (port 8443 in Docker)
  - gRPC server with TLS for node registration and data services
  - Optional client certificate verification for mTLS
  - Uses `TLS13_AES_256_GCM_SHA384` cipher suite

- **Storage Node TLS Client**
  - TLS client connections to gateway (https://)
  - Client certificate authentication (mTLS)
  - Configurable via environment variables or config file

- **CLI TLS Support**
  - HTTPS connections with custom CA certificate
  - Client certificate authentication for secure operations
  - `--ca-cert`, `--client-cert`, `--client-key` CLI options

- **Shared TLS Module** (`cyxcloud-core/src/tls.rs`)
  - `TlsServerConfig` - Server TLS configuration
  - `TlsClientConfig` - Client TLS configuration
  - `create_tonic_server_tls()` - gRPC server TLS setup
  - `create_tonic_client_tls()` - gRPC client TLS setup

- **Certificate Generation Script** (`scripts/generate-dev-certs.sh`)
  - Generates self-signed CA and certificates for development
  - Creates gateway, node, and CLI certificates
  - Outputs to `certs/` directory

- **Docker TLS Overlay** (`docker-compose.tls.yml`)
  - TLS configuration overlay for Docker Compose
  - Mounts certificates into containers
  - Configures TLS environment variables

### Changed

- Updated `rustls` to 0.23 with explicit `ring` crypto provider
- Gateway and node now require crypto provider initialization at startup
- Node registration uses HTTPS (`https://gateway:50052`) when TLS enabled

### Fixed

- Fixed rustls 0.23+ panic: "Could not automatically determine CryptoProvider"
  - Added explicit `rustls::crypto::ring::default_provider().install_default()` call

### Environment Variables

| Variable | Component | Description |
|----------|-----------|-------------|
| `TLS_CERT` | Gateway | Server certificate path |
| `TLS_KEY` | Gateway | Server private key path |
| `TLS_CA_CERT` | All | CA certificate for verification |
| `TLS_CLIENT_CERT` | Node, CLI | Client certificate for mTLS |
| `TLS_CLIENT_KEY` | Node, CLI | Client private key |
| `TLS_ENABLED` | Node | Enable TLS connections |
| `TLS_REQUIRE_CLIENT_CERT` | Gateway | Require client certificates |

### Docker Usage

```bash
# Generate development certificates
./scripts/generate-dev-certs.sh

# Start with TLS enabled
docker compose -f docker-compose.yml -f docker-compose.tls.yml up -d

# Test HTTPS gateway
curl -k https://localhost:8443/health
```

---

## [0.1.0] - 2025-12-29

### Added

- Initial release of CyxCloud distributed storage system
- Gateway with S3-compatible REST API
- Storage nodes with erasure coding (Reed-Solomon)
- gRPC-based node registration and heartbeat
- PostgreSQL metadata storage
- Redis caching layer
- Rebalancer daemon for chunk distribution
- Payment daemon for node compensation
- Node lifecycle monitor
- Docker Compose deployment
- CLI tool for file operations
- JWT authentication
- Wallet-based node identity
