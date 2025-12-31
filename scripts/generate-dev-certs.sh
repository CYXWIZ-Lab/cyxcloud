#!/bin/bash
# CyxCloud Development Certificate Generator
# Generates self-signed CA and certificates for mTLS testing
#
# Usage: ./scripts/generate-dev-certs.sh [output_dir]
# Default output: ./certs/

set -e

CERT_DIR="${1:-./certs}"
DAYS_VALID=365
KEY_SIZE=2048
CA_KEY_SIZE=4096

echo "==================================="
echo "CyxCloud Certificate Generator"
echo "==================================="
echo "Output directory: $CERT_DIR"
echo ""

# Create output directory
mkdir -p "$CERT_DIR"

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

print_status() {
    echo -e "${GREEN}[OK]${NC} $1"
}

print_warning() {
    echo -e "${YELLOW}[WARN]${NC} $1"
}

# Check if openssl is available
if ! command -v openssl &> /dev/null; then
    echo -e "${RED}[ERROR]${NC} OpenSSL is not installed. Please install it first."
    exit 1
fi

echo "Step 1: Generating Root CA..."
echo "-----------------------------------"

# Generate CA private key
openssl genrsa -out "$CERT_DIR/ca.key" $CA_KEY_SIZE 2>/dev/null
print_status "Generated CA private key: ca.key"

# Generate CA certificate
openssl req -x509 -new -nodes \
    -key "$CERT_DIR/ca.key" \
    -sha256 \
    -days $DAYS_VALID \
    -out "$CERT_DIR/ca.crt" \
    -subj "/C=US/ST=California/L=San Francisco/O=CyxCloud/OU=Development/CN=CyxCloud-Dev-CA"
print_status "Generated CA certificate: ca.crt"

echo ""
echo "Step 2: Generating Gateway Certificate..."
echo "-----------------------------------"

# Create gateway extension config for SANs
cat > "$CERT_DIR/gateway-ext.cnf" << EOF
[req]
default_bits = $KEY_SIZE
distinguished_name = req_distinguished_name
req_extensions = v3_req
prompt = no

[req_distinguished_name]
C = US
ST = California
L = San Francisco
O = CyxCloud
OU = Gateway
CN = gateway

[v3_req]
basicConstraints = CA:FALSE
keyUsage = nonRepudiation, digitalSignature, keyEncipherment
extendedKeyUsage = serverAuth, clientAuth
subjectAltName = @alt_names

[alt_names]
DNS.1 = gateway
DNS.2 = localhost
DNS.3 = *.cyxcloud-net
DNS.4 = cyxcloud-gateway
IP.1 = 127.0.0.1
IP.2 = ::1
EOF

# Generate gateway private key
openssl genrsa -out "$CERT_DIR/gateway.key" $KEY_SIZE 2>/dev/null
print_status "Generated gateway private key: gateway.key"

# Generate gateway CSR
openssl req -new \
    -key "$CERT_DIR/gateway.key" \
    -out "$CERT_DIR/gateway.csr" \
    -config "$CERT_DIR/gateway-ext.cnf"
print_status "Generated gateway CSR"

# Sign gateway certificate with CA
openssl x509 -req \
    -in "$CERT_DIR/gateway.csr" \
    -CA "$CERT_DIR/ca.crt" \
    -CAkey "$CERT_DIR/ca.key" \
    -CAcreateserial \
    -out "$CERT_DIR/gateway.crt" \
    -days $DAYS_VALID \
    -sha256 \
    -extensions v3_req \
    -extfile "$CERT_DIR/gateway-ext.cnf" 2>/dev/null
print_status "Generated gateway certificate: gateway.crt"

echo ""
echo "Step 3: Generating Node Certificates..."
echo "-----------------------------------"

# Generate certificates for 3 nodes
for i in 1 2 3; do
    NODE_NAME="node$i"
    NODE_ID="node-$i"

    # Create node extension config
    cat > "$CERT_DIR/${NODE_NAME}-ext.cnf" << EOF
[req]
default_bits = $KEY_SIZE
distinguished_name = req_distinguished_name
req_extensions = v3_req
prompt = no

[req_distinguished_name]
C = US
ST = California
L = San Francisco
O = CyxCloud
OU = Storage Nodes
CN = $NODE_ID

[v3_req]
basicConstraints = CA:FALSE
keyUsage = nonRepudiation, digitalSignature, keyEncipherment
extendedKeyUsage = serverAuth, clientAuth
subjectAltName = @alt_names

[alt_names]
DNS.1 = $NODE_NAME
DNS.2 = $NODE_ID
DNS.3 = localhost
IP.1 = 127.0.0.1
EOF

    # Generate node private key
    openssl genrsa -out "$CERT_DIR/${NODE_NAME}.key" $KEY_SIZE 2>/dev/null

    # Generate node CSR
    openssl req -new \
        -key "$CERT_DIR/${NODE_NAME}.key" \
        -out "$CERT_DIR/${NODE_NAME}.csr" \
        -config "$CERT_DIR/${NODE_NAME}-ext.cnf"

    # Sign node certificate with CA
    openssl x509 -req \
        -in "$CERT_DIR/${NODE_NAME}.csr" \
        -CA "$CERT_DIR/ca.crt" \
        -CAkey "$CERT_DIR/ca.key" \
        -CAcreateserial \
        -out "$CERT_DIR/${NODE_NAME}.crt" \
        -days $DAYS_VALID \
        -sha256 \
        -extensions v3_req \
        -extfile "$CERT_DIR/${NODE_NAME}-ext.cnf" 2>/dev/null

    print_status "Generated certificate for $NODE_NAME"
done

echo ""
echo "Step 4: Generating CLI Client Certificate..."
echo "-----------------------------------"

# Create CLI extension config
cat > "$CERT_DIR/cli-ext.cnf" << EOF
[req]
default_bits = $KEY_SIZE
distinguished_name = req_distinguished_name
req_extensions = v3_req
prompt = no

[req_distinguished_name]
C = US
ST = California
L = San Francisco
O = CyxCloud
OU = CLI Clients
CN = cli-client

[v3_req]
basicConstraints = CA:FALSE
keyUsage = nonRepudiation, digitalSignature, keyEncipherment
extendedKeyUsage = clientAuth
EOF

# Generate CLI private key
openssl genrsa -out "$CERT_DIR/cli.key" $KEY_SIZE 2>/dev/null
print_status "Generated CLI private key: cli.key"

# Generate CLI CSR
openssl req -new \
    -key "$CERT_DIR/cli.key" \
    -out "$CERT_DIR/cli.csr" \
    -config "$CERT_DIR/cli-ext.cnf"
print_status "Generated CLI CSR"

# Sign CLI certificate with CA
openssl x509 -req \
    -in "$CERT_DIR/cli.csr" \
    -CA "$CERT_DIR/ca.crt" \
    -CAkey "$CERT_DIR/ca.key" \
    -CAcreateserial \
    -out "$CERT_DIR/cli.crt" \
    -days $DAYS_VALID \
    -sha256 \
    -extensions v3_req \
    -extfile "$CERT_DIR/cli-ext.cnf" 2>/dev/null
print_status "Generated CLI certificate: cli.crt"

echo ""
echo "Step 5: Setting permissions and cleanup..."
echo "-----------------------------------"

# Set restrictive permissions on private keys
chmod 600 "$CERT_DIR"/*.key
print_status "Set restrictive permissions on private keys"

# Clean up CSR and config files
rm -f "$CERT_DIR"/*.csr "$CERT_DIR"/*.cnf "$CERT_DIR"/*.srl
print_status "Cleaned up temporary files"

echo ""
echo "==================================="
echo "Certificate Generation Complete!"
echo "==================================="
echo ""
echo "Generated files in $CERT_DIR:"
echo "  CA:      ca.crt, ca.key"
echo "  Gateway: gateway.crt, gateway.key"
echo "  Nodes:   node1.crt/key, node2.crt/key, node3.crt/key"
echo "  CLI:     cli.crt, cli.key"
echo ""
echo "Usage:"
echo "  Gateway:  --tls-cert $CERT_DIR/gateway.crt --tls-key $CERT_DIR/gateway.key --tls-ca-cert $CERT_DIR/ca.crt"
echo "  Node:     --tls-cert $CERT_DIR/node1.crt --tls-key $CERT_DIR/node1.key --tls-ca-cert $CERT_DIR/ca.crt"
echo "  CLI:      --ca-cert $CERT_DIR/ca.crt --client-cert $CERT_DIR/cli.crt --client-key $CERT_DIR/cli.key"
echo ""
print_warning "These certificates are for DEVELOPMENT ONLY. Use proper CA-signed certificates in production!"
