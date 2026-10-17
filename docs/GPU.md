# Whisper on the graphics card (Windows, Vulkan)

whisper.cpp publishes no Vulkan build for Windows, so it's built locally and
copied to `%LOCALAPPDATA%\SrutamDictate\whisper\runtime-vulkan`. When that
folder has `whisper-server.exe`, the app runs Whisper there (kept loaded,
full 30 s window); otherwise it falls back to the CPU `whisper-cli`.
`SRUTAM_WHISPER_CPU=1` forces the CPU path.

Measured on a Ryzen 5 5600H + RX 6500M (4 GB), Whisper large-v3-turbo q5_0:

| Clip | CPU (whisper-cli) | GPU (whisper-server) |
|---|---|---|
| ~2.5 s | 3.6-4.9 s | ~1.5 s |
| 17-26 s | ~11 s | ~1.7 s |
| 50-80 s | 22-33 s | 3.6-7 s |

Uses ~820 MB of graphics memory (Qwen3 0.6B for the AI clean-up: ~860 MB).

## Picking the card

Laptops often have two GPUs; whisper.cpp and llama.cpp list the built-in one
first. `src-tauri/src/gpu.rs` finds the dedicated card with a Vulkan query and
starts both servers with `GGML_VK_VISIBLE_DEVICES=<its index>`, so they see
only that card.

## Building it (no Vulkan SDK install needed)

Needs Visual Studio (C++ tools), CMake, Ninja and Python. Everything else goes
in one folder (here `F:\VulkanKit`):

- `git clone --depth 1 https://github.com/KhronosGroup/Vulkan-Headers.git`
- `git clone --depth 1 https://github.com/KhronosGroup/SPIRV-Headers.git`
- `git clone --depth 1 --branch v1.9.5 https://github.com/ggml-org/whisper.cpp.git`
- `glslc.exe`: the Android NDK has one (`ndk\<version>\shader-tools\windows-x86_64`).

Then, in a VS x64 developer prompt (`vcvars64.bat`):

```bat
:: Import library for the system's Vulkan loader
dumpbin /nologo /exports C:\Windows\System32\vulkan-1.dll > exports.txt
python -c "import re;n=[m.group(1) for l in open('exports.txt') for m in [re.match(r'\s+\d+\s+[0-9A-F]+\s+[0-9A-F]+\s+(\w+)',l)] if m];open('vulkan-1.def','w').write('LIBRARY vulkan-1\nEXPORTS\n'+'\n'.join(n)+'\n')"
lib /nologo /def:vulkan-1.def /out:vulkan-1.lib /machine:x64

cmake -S SPIRV-Headers -B SPIRV-Headers/build -G Ninja -DSPIRV_HEADERS_ENABLE_TESTS=OFF -DSPIRV_HEADERS_ENABLE_INSTALL=ON
cmake --install SPIRV-Headers/build --prefix F:/VulkanKit/spirv

cmake -S whisper.cpp -B build -G Ninja -DCMAKE_BUILD_TYPE=Release -DBUILD_SHARED_LIBS=ON ^
  -DGGML_VULKAN=ON -DGGML_NATIVE=ON -DWHISPER_BUILD_TESTS=OFF -DWHISPER_SDL2=OFF ^
  -DCMAKE_PREFIX_PATH=F:/VulkanKit/spirv "-DCMAKE_CXX_FLAGS=/IF:/VulkanKit/spirv/include /EHsc" ^
  -DVulkan_INCLUDE_DIR=F:/VulkanKit/Vulkan-Headers/include -DVulkan_LIBRARY=F:/VulkanKit/vulkan-1.lib ^
  -DVulkan_GLSLC_EXECUTABLE=<path to glslc.exe>
cmake --build build --config Release -j 10 --target whisper-cli whisper-server
```

Copy `build\bin\{whisper-server.exe, whisper-cli.exe, whisper.dll, ggml*.dll}`
to `runtime-vulkan`.

`GGML_NATIVE=ON` targets this CPU only, and the DLLs need the Visual C++
runtime. A build for other people needs `GGML_NATIVE=OFF` and a hosted
download, the way the CPU runtime is fetched.
