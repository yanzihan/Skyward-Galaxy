#!/bin/bash
# 交叉编译 Skyward 主程序(aarch64-linux-android)
set -u
export PATH="$HOME/.cargo/bin:$PATH"
NDK="/home/yanzihan/文档/deepseek-harness/default-workspace/.toolchain/android-ndk-r29"
CLANG="$NDK/toolchains/llvm/prebuilt/linux-x86_64/bin/aarch64-linux-android30-clang"
export ANDROID_NDK_HOME="$NDK"
export CC_aarch64_linux_android="$CLANG"
export AR_aarch64_linux_android="$NDK/toolchains/llvm/prebuilt/linux-x86_64/bin/llvm-ar"
export CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER="$CLANG"
cd /home/yanzihan/build2/skyward
cargo build --release --target aarch64-linux-android -p skyward
echo "SKYWARD_EXIT_$?"
