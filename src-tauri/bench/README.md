# Hinglish benchmark

Real Hindi-English code-switched speech for comparing the dictation models on
the same clips. The audio is not committed (see .gitignore); the runner and
this README are.

## Clips

- `iacc01–08.wav` — from the HiACC corpus (Zenodo doi
  10.5281/zenodo.15551669, CC BY 4.0). Adult speakers answering everyday
  questions (places, festivals, family, travel) spontaneously in Hinglish.
  Chosen for strong code-switching. 8–16 s each.
- `slr01–10.wav` — cut from MUCS 2021 Hindi-English test set (OpenSLR SLR-104,
  CC BY-SA 4.0). Spoken-tutorial presenters explaining technical topics;
  presentation-style code-switching ("slide number 10 checker click kijiye").
  Cut using the official Kaldi `segments`/`text` files. 8–14 s each.

All converted to 16 kHz mono 16-bit WAV, the same format dictation feeds the
models.
  
## References

`refs.csv` maps each clip to its official reference transcription
(file, source, duration_s, reference; UTF-8). HiACC and MUCS references are
Devanagari with English words in Latin script. Use them for eyeballing and
for WER once a script-normalising checker exists.

## Running

From `src-tauri/`:

```sh
powershell -ExecutionPolicy Bypass -File bench\run-benchmark.ps1
```

Each installed model (parakeet-v3, whisper-tiny, hinglish-apex,
hinglish-swift) runs over every clip; output lands in `bench\results-<id>.txt`
with per-clip decode time and the raw/rules/dict text.

Single model by hand:

```sh
$env:SRUTAM_TEST_WAV = "bench/hinglish"
$env:SRUTAM_TEST_MODEL = "hinglish-apex"
cargo test --release --lib transcribes_wav_with_installed_model -- --ignored --nocapture
```

## Regenerating the set

The source archives were fetched once (HiACC `Corpus.zip` from Zenodo;
SLR-104 `Hindi-English_test.tar.gz` from openslr.trmal.net) and kept outside
the repo. Re-run the curation steps above if the bench folder is lost.
