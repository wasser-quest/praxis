#!/bin/bash

# Qwen3-TTS HTTP Server
# Setup and start script

set -e

echo "========================================"
echo "Qwen3-TTS HTTP Server"
echo "========================================"
echo ""

# Get script directory
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR"

# Check Python
if ! command -v python3 &> /dev/null; then
    echo "Error: Python 3 is not installed"
    exit 1
fi

echo "Python version: $(python3 --version)"

# Create virtual environment if it doesn't exist or is incomplete
VENV_PYTHON="venv/bin/python3"
if [ ! -f "$VENV_PYTHON" ]; then
    echo "Creating virtual environment..."
    rm -rf venv
    python3 -m venv venv
fi

# Activate virtual environment
echo "Activating virtual environment..."
source venv/bin/activate

# Upgrade pip
echo "Upgrading pip..."
pip install --upgrade pip

# Install requirements
echo "Installing Qwen3-TTS..."
pip install qwen-tts soundfile

# Force reinstall PyTorch with CUDA support (must be after qwen-tts to override CPU-only torch)
echo "Installing PyTorch with CUDA support..."
pip install --force-reinstall torch torchaudio --index-url https://download.pytorch.org/whl/cu124

# Check for GPU
echo ""
echo "Checking GPU..."
python3 -c "import torch; print(f'CUDA available: {torch.cuda.is_available()}')" 2>/dev/null || echo "CUDA check failed (may need GPU drivers)"

# Set environment variables
export OMP_NUM_THREADS=4

echo ""
echo "========================================"
echo "Starting Qwen3-TTS server"
echo "Server will run on http://localhost:8001"
echo "Press Ctrl+C to stop"
echo "========================================"
echo ""

# Start the Qwen3-TTS server
python3 qwen3_tts_server.py --model Qwen/Qwen3-TTS-12Hz-1.7B-Base --type Base --port 8001
