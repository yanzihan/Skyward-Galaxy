#!/bin/bash
# 通用打包脚本:给任意三星机型出 Skyward recovery 镜像
# 用法: build-model.sh <代号> <Fox 的 recovery.img> <recovery 分区大小> [额外的 --add-file 参数...]
#   devices/<代号>.toml 必须存在;分区大小取 BoardConfig.mk 的
#   BOARD_RECOVERYIMAGE_PARTITION_SIZE(或官方 recovery.img 的长度)。
set -euo pipefail
export PATH="$HOME/.cargo/bin:$PATH"
C=/home/yanzihan/build2/skyward
AVB=/home/yanzihan/android/fox_16.0/external/avb/avbtool.py
KEYS=$C/work/avb-keys

CODE=${1:?需要代号, 例如 y2q}
FOX=${2:?需要 Fox 的 recovery.img}
PART=${3:?需要 recovery 分区大小(字节)}
shift 3
EXTRA=("$@")   # 形如 --add-file "host=path"

CFG=$C/devices/$CODE.toml
[ -f "$CFG" ] || { echo "缺少设备配置 $CFG"; exit 1; }
WORK=$C/work/$CODE
mkdir -p "$WORK"

# 输出名 Skyward-Galaxy_<Cargo.toml 里的版本>-<代号>.后缀;再编加 _01、_02
mkdir -p "$C/target"
VER=$(sed -n 's/^version = "\(.*\)"$/\1/p' "$C/Cargo.toml" | head -1)
BASE="Skyward-Galaxy_${VER}-${CODE}"
N=0
while :; do
  TAG=''
  [ "$N" -gt 0 ] && TAG=$(printf -- '_%02d' "$N")
  if [ ! -e "$C/target/$BASE$TAG.img" ] && [ ! -e "$C/target/$BASE$TAG.tar.md5" ]; then
    break
  fi
  N=$((N + 1))
done
OUT="$C/target/$BASE$TAG"
# 默认用仓库里未修改的 MiSans;FONT=... 可临时换一款(比如空间不够的测试版)
FONT=${FONT:-$C/resources/fonts/MiSansVF.ttf}

$C/target/release/skyward-pack recovery --stock "$FOX" \
  --binary $C/target/aarch64-linux-android/release/skyward \
  --system-init $C/target/system-init/aarch64-linux-android/release/system-init \
  --config "$CFG" --font "$FONT" \
  --mono $C/resources/fonts/KirkMono-Regular.ttf --mono-license $C/resources/fonts/KirkMono-OFL.txt \
  --third-party --no-mtp --partition-size "$PART" \
  "${EXTRA[@]}" --out "$WORK/base.img" | tail -1

# 取打包器自己记录的镜像长度,去掉它按基底写的 footer
python3 - "$WORK/base.img" "$WORK/raw.img" <<'PY'
import struct, sys
d = open(sys.argv[1], 'rb').read()
if d[-64:-60] != b'AVBf':
    sys.exit('基底尾部没有 AVBf')
n = struct.unpack('>Q', d[-52:-44])[0]
open(sys.argv[2], 'wb').write(d[:n])
print(f'content {n} bytes')
PY

python3 "$AVB" add_hash_footer --image "$WORK/raw.img" \
  --partition_size "$PART" --partition_name recovery \
  --algorithm SHA256_RSA4096 --key "$KEYS/skyward-4096.pem" \
  --salt "$(printf '6a%.0s' {1..32})" --internal_release_string skyward-pack >/dev/null
mv "$WORK/raw.img" "$OUT.img"

cd "$C/target"
cp "$OUT.img" recovery.img
# odin4 不认 md5 行里的绝对路径:名字要和 tar 一样,只用文件名
TAR=$(basename "$OUT.tar")
tar cf "$TAR" recovery.img
md5sum -t "$TAR" >> "$TAR"
mv "$TAR" "$TAR.md5"
rm -f recovery.img
ls -la "$OUT.img" "$OUT.tar.md5"
