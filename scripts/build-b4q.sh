#!/bin/bash
# Build the b4q recovery image: the hybrid base (YanJiaMian's kernel
# 5.10.274-YanJiaMian-gki, FIVE/PROCA off, + the V2 TWRP ramdisk) carries the
# driver modules, and the QCA6490 firmware goes into the ramdisk for the
# wireless chip's cold boot calibration.
set -eu
C=/home/yanzihan/build2/skyward
X=$C/work/b4q
# 基底放在子目录里:build-model.sh 会把打包结果写到 work/<代号>/base.img,放在
# 同一层会被它覆盖,下一次打包就对着一个已经装好 Skyward 的镜像再打一遍。
BASE=$X/base/hybrid.img

ARGS=()
for f in $X/qca6490/*; do
  ARGS+=(--add-file "$f=lib/firmware/qca6490/$(basename $f)")
done

bash $C/scripts/build-model.sh b4q "$BASE" 104857600 "${ARGS[@]}" "$@"
