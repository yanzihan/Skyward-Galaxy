#!/bin/bash
# Build the p3q recovery image with everything the device needs: the dhd
# module and its firmware, the VNDK wifi libraries, and the touch firmware
# the kernel asks for before init starts (without it, the boot waits out
# the 60-second firmware timeout).
set -eu
C=/home/yanzihan/build2/skyward
X=$C/work/p3q-extra

bash $C/scripts/build-model.sh p3q $C/work/p3q-fox/recovery.img 81788928 \
  --add-file $X/lib/modules/dhd.ko=lib/modules/dhd.ko \
  --add-file $X/lib/modules/modules.dep=lib/modules/modules.dep \
  --add-file $X/system/etc/wifi/bcmdhd_sta.bin=system/etc/wifi/bcmdhd_sta.bin \
  --add-file $X/system/etc/wifi/bcmdhd_clm.blob=system/etc/wifi/bcmdhd_clm.blob \
  --add-file $X/system/etc/wifi/nvram_net.txt=system/etc/wifi/nvram_net.txt \
  --add-file "$X/system/wifi-vndk/lib64/android.hardware.wifi@1.0.so=system/wifi-vndk/lib64/android.hardware.wifi@1.0.so" \
  --add-file "$X/system/wifi-vndk/lib64/android.hardware.wifi.supplicant@1.0.so=system/wifi-vndk/lib64/android.hardware.wifi.supplicant@1.0.so" \
  --add-file "$X/system/wifi-vndk/lib64/android.hardware.wifi.supplicant@1.1.so=system/wifi-vndk/lib64/android.hardware.wifi.supplicant@1.1.so" \
  --add-file "$X/system/wifi-vndk/lib64/android.hardware.wifi.supplicant@1.2.so=system/wifi-vndk/lib64/android.hardware.wifi.supplicant@1.2.so" \
  --add-file "$X/system/wifi-vndk/lib64/android.hardware.wifi.supplicant@1.3.so=system/wifi-vndk/lib64/android.hardware.wifi.supplicant@1.3.so" \
  --add-file "$X/system/wifi-vndk/lib64/libssl.so=system/wifi-vndk/lib64/libssl.so" \
  --add-file "$X/system/wifi-vndk/lib64/libcrypto.so=system/wifi-vndk/lib64/libcrypto.so" \
  --add-file "$X/system/wifi-vndk/lib64/liblog.so=system/wifi-vndk/lib64/liblog.so" \
  --add-file $X/lib/firmware/y792_p3.bin=lib/firmware/y792_p3.bin \
  "$@"
