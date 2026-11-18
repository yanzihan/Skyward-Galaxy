#!/bin/bash
# 打 y2q 的 Skyward 镜像: Fox 基底 + Skyward + 按 y2q 分区大小签名
set -euo pipefail
export PATH="$HOME/.cargo/bin:$PATH"
C=/home/yanzihan/build2/skyward
AVB=/home/yanzihan/android/fox_16.0/external/avb/avbtool.py
KEYS=$C/work/avb-keys
PART=82694144   # BOARD_RECOVERYIMAGE_PARTITION_SIZE

# 输出名 Skyward-Galaxy_<Cargo.toml 里的版本>-y2q.后缀;再编加 _01、_02
mkdir -p "$C/target"
VER=$(sed -n 's/^version = "\(.*\)"$/\1/p' "$C/Cargo.toml" | head -1)
BASE="Skyward-Galaxy_${VER}-y2q"
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

$C/target/release/skyward-pack recovery --stock $C/work/y2q/recovery.img \
  --binary $C/target/aarch64-linux-android/release/skyward \
  --system-init $C/target/system-init/aarch64-linux-android/release/system-init \
  --config $C/devices/y2q.toml --font $C/resources/fonts/MiSansVF.ttf \
  --mono $C/resources/fonts/KirkMono-Regular.ttf --mono-license $C/resources/fonts/KirkMono-OFL.txt \
  --third-party --no-mtp --partition-size "$PART" --out $C/work/y2q/base.img | tail -1

# 去掉打包器按基底大小写的 footer,取它自己记录的镜像长度
python3 - "$C/work/y2q/base.img" "$C/work/y2q/raw.img" <<'PY'
import struct, sys
d = open(sys.argv[1], 'rb').read()
if d[-64:-60] != b'AVBf':
    sys.exit('基底尾部没有 AVBf')
n = struct.unpack('>Q', d[-52:-44])[0]
open(sys.argv[2], 'wb').write(d[:n])
print(f'content {n} bytes')
PY

python3 "$AVB" add_hash_footer --image "$C/work/y2q/raw.img" \
  --partition_size "$PART" --partition_name recovery \
  --algorithm SHA256_RSA4096 --key "$KEYS/skyward-4096.pem" \
  --salt "$(printf '68%.0s' {1..32})" --internal_release_string skyward-pack >/dev/null
mv "$C/work/y2q/raw.img" "$OUT.img"

cd "$C/target"
cp "$OUT.img" recovery.img
# odin4 不认 md5 行里的绝对路径:名字要和 tar 一样,只用文件名
TAR=$(basename "$OUT.tar")
tar cf "$TAR" recovery.img
md5sum -t "$TAR" >> "$TAR"
mv "$TAR" "$TAR.md5"
rm -f recovery.img
ls -la "$OUT.img" "$OUT.tar.md5"
