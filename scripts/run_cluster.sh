#!/bin/bash
#
# CyxCloud Cluster Test Script (Linux/macOS)
#
# Usage:
#   ./run_cluster.sh           # Start 3-node cluster
#   ./run_cluster.sh -n 5      # Start 5-node cluster
#   ./run_cluster.sh -t        # Start cluster and run tests
#   ./run_cluster.sh -b        # Build first, then start cluster
#   ./run_cluster.sh -c        # Clean data before starting

set -e

# Default values
NODES=3
TEST=false
BUILD=false
CLEAN=false

# Parse arguments
while getopts "n:tbc" opt; do
    case $opt in
        n) NODES=$OPTARG ;;
        t) TEST=true ;;
        b) BUILD=true ;;
        c) CLEAN=true ;;
        \?) echo "Invalid option: -$OPTARG" >&2; exit 1 ;;
    esac
done

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"
DATA_ROOT="$PROJECT_ROOT/cluster_data"
BASE_PORT=50051

echo "CyxCloud Cluster Test"
echo "====================="
echo "Nodes: $NODES"
echo "Data directory: $DATA_ROOT"
echo ""

# Clean up old data if requested
if [ "$CLEAN" = true ]; then
    echo "Cleaning up old cluster data..."
    rm -rf "$DATA_ROOT"
fi

# Build if requested
if [ "$BUILD" = true ]; then
    echo "Building cluster_node example..."
    cd "$PROJECT_ROOT"
    cargo build --example cluster_node --release
    echo "Build complete!"
    echo ""
fi

# Check if binary exists
BINARY_PATH="$PROJECT_ROOT/target/release/examples/cluster_node"
if [ ! -f "$BINARY_PATH" ]; then
    echo "Binary not found. Run with -b flag first."
    echo "  ./run_cluster.sh -b"
    exit 1
fi

# Create data directories
mkdir -p "$DATA_ROOT"

# Array to hold PIDs
declare -a PIDS
declare -a PORTS

# Cleanup function
cleanup() {
    echo ""
    echo "Stopping all nodes..."
    for pid in "${PIDS[@]}"; do
        if kill -0 "$pid" 2>/dev/null; then
            kill "$pid" 2>/dev/null || true
        fi
    done
    echo "Cluster stopped."
    exit 0
}

# Set trap for cleanup
trap cleanup SIGINT SIGTERM

# Start nodes
for i in $(seq 1 $NODES); do
    PORT=$((BASE_PORT + i - 1))
    PORTS+=($PORT)
    NODE_ID="node$i"
    NODE_DATA_DIR="$DATA_ROOT/$NODE_ID"
    mkdir -p "$NODE_DATA_DIR"

    TEST_FLAG=""
    if [ "$TEST" = true ] && [ $i -eq 1 ]; then
        TEST_FLAG="--test"
    fi

    # First node has no bootstrap, others bootstrap from first node
    BOOTSTRAP_FLAG=""
    if [ $i -gt 1 ]; then
        BOOTSTRAP_FLAG="--bootstrap 127.0.0.1:$BASE_PORT"
    fi

    echo "Starting $NODE_ID on port $PORT..."

    $BINARY_PATH --port $PORT --data-dir "$NODE_DATA_DIR" --node-id $NODE_ID $TEST_FLAG $BOOTSTRAP_FLAG &
    PIDS+=($!)

    # Give each node time to start
    sleep 0.5
done

echo ""
echo "Cluster started with $NODES nodes!"
echo "Ports: ${PORTS[*]}"
echo ""
echo "Press Ctrl+C to stop all nodes..."

# Wait for all processes
wait
