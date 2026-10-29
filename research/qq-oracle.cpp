// Development-only reference oracle. Never linked into or shipped with engine.exe.
// KernelUtil.dll 9.7.25.29417 SHA256 b39f79c6cf43a5665dabb587f31784c713fbfda55ef0ebdd4a51ac1d0a0ec4e9
#include <windows.h>
#include <cstdint>
#include <fstream>
#include <vector>
#include <filesystem>
#include <iostream>
int wmain(int argc, wchar_t** argv) {
    if (argc != 4) return 2;
    auto libraryPath = std::filesystem::path(argv[1]);
    SetDllDirectoryW(libraryPath.parent_path().c_str());
    HMODULE module = LoadLibraryW(libraryPath.c_str());
    if (!module) return 3;
    auto transform = reinterpret_cast<int(__cdecl*)(uint32_t*, int, const uint32_t*)>(reinterpret_cast<uint8_t*>(module) + 0x90f78);
    uint32_t key[4] = {0x42424242, 0x42424242, 0x42424242, 0x42424242};
    std::ifstream input(std::filesystem::path(argv[2]), std::ios::binary);
    std::ofstream output(std::filesystem::path(argv[3]), std::ios::binary);
    if (!input || !output) return 4;
    std::vector<uint32_t> block(2048);
    while (input.read(reinterpret_cast<char*>(block.data()), 8192)) {
        if (transform(block.data(), 2048, key) != 0) return 5;
        output.write(reinterpret_cast<const char*>(block.data()), 8192);
    }
    if (input.gcount() != 0 || !output) return 6;
    std::cout << "oracle complete\n";
    return 0;
}
