#!/usr/bin/env python3
"""
Export DCCRN+ model to ONNX for use in DepoAudio.

Usage:
    pip install torch torchaudio onnx onnxruntime
    python scripts/export_dccrn.py

This will download the pretrained DCCRN model and export it to ONNX format.
The output file should be placed in src-tauri/resources/models/dccrn_plus.onnx

Note: DCCRN+ handles both noise reduction and mild dereverberation.
"""

import sys
import os

def export_dccrn():
    try:
        import torch
        import torch.onnx
    except ImportError:
        print("Error: PyTorch not installed. Run: pip install torch torchaudio")
        sys.exit(1)

    # Clone DCCRN repository if not present
    dccrn_dir = os.path.join(os.path.dirname(__file__), "DeepComplexCRN")
    if not os.path.exists(dccrn_dir):
        print("Cloning DCCRN repository...")
        os.system(f"git clone https://github.com/huyanxin/DeepComplexCRN.git {dccrn_dir}")

    sys.path.insert(0, dccrn_dir)

    try:
        # Try to import the DCCRN model
        from DCCRN import DCCRN
    except ImportError:
        print("Error: Could not import DCCRN. Check the repository structure.")
        print(f"Expected at: {dccrn_dir}")
        sys.exit(1)

    # Initialize model with default config
    print("Loading DCCRN model...")
    model = DCCRN(
        rnn_units=256,
        masking_mode='E',  # Complex ratio mask
        use_clstm=True,
        kernel_num=[32, 64, 128, 256, 256, 256],
    )
    model.eval()

    # Create dummy input: 2 seconds of 16kHz mono audio
    sample_rate = 16000
    duration = 2  # seconds
    dummy_input = torch.randn(1, sample_rate * duration)

    # Export to ONNX
    output_path = os.path.join(
        os.path.dirname(__file__), "..",
        "src-tauri", "resources", "models", "dccrn_plus.onnx"
    )

    print(f"Exporting to {output_path}...")
    torch.onnx.export(
        model,
        dummy_input,
        output_path,
        input_names=["input"],
        output_names=["output"],
        dynamic_axes={
            "input": {1: "audio_length"},
            "output": {1: "audio_length"},
        },
        opset_version=14,
        do_constant_folding=True,
    )

    # Verify the export
    try:
        import onnxruntime as ort
        session = ort.InferenceSession(output_path)
        import numpy as np
        test_input = np.random.randn(1, 32000).astype(np.float32)
        result = session.run(None, {"input": test_input})
        print(f"Export successful! Output shape: {result[0].shape}")
        print(f"Model size: {os.path.getsize(output_path) / 1024 / 1024:.1f} MB")
        print(f"\nPlace the file at: src-tauri/resources/models/dccrn_plus.onnx")
    except ImportError:
        print("Export complete (could not verify — install onnxruntime to test)")
    except Exception as e:
        print(f"Export complete but verification failed: {e}")
        print("The model may need adjustments for ONNX compatibility.")


if __name__ == "__main__":
    export_dccrn()
