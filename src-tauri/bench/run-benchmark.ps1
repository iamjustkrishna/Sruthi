# Runs the Hinglish benchmark clips through each installed model and saves
# output to bench\results-<model>.txt. Run from src-tauri:
#
#   powershell -ExecutionPolicy Bypass -File bench\run-benchmark.ps1
#
# Builds once up front so the four runs reuse the same binary.

$MODELS = @("parakeet-v3", "whisper-tiny", "hinglish-swift", "hinglish-apex")

"=== cargo build --release --tests"
cargo build --release --tests
if ($LASTEXITCODE -ne 0) { "BUILD FAILED ($LASTEXITCODE)"; exit 1 }

foreach ($m in $MODELS) {
    $env:SRUTHI_TEST_WAV = "bench/hinglish"
    $env:SRUTHI_TEST_MODEL = $m
    $out = "bench\results-$m.txt"
    "=== $m -> $out"
    cargo test --release --lib transcribes_wav_with_installed_model -- --ignored --nocapture *>&1 | Out-File $out -Encoding utf8
    "    exit $LASTEXITCODE, $((Get-Item $out).Length) bytes"
}

Remove-Item Env:SRUTHI_TEST_MODEL, Env:SRUTHI_TEST_WAV -ErrorAction SilentlyContinue
"All results in bench\results-*.txt"
