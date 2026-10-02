# MSVC + Windows SDK environment for building this project.
# `vcvars64.bat` is the canonical source, but it is slow and can hang while the VS
# installer is still finishing. These paths are stable for the toolchain installed at
# C:\BuildTools, so we set them directly.
MSVC_ROOT="/c/BuildTools/VC/Tools/MSVC/14.44.35207"
SDK_ROOT="/c/Program Files (x86)/Windows Kits/10"
SDK_VER="10.0.26100.0"

export PATH="$USERPROFILE/.cargo/bin:$MSVC_ROOT/bin/Hostx64/x64:$PATH"
export LIB="C:\BuildTools\VC\Tools\MSVC\14.44.35207\lib\x64;C:\Program Files (x86)\Windows Kits\10\Lib\10.0.26100.0\ucrt\x64;C:\Program Files (x86)\Windows Kits\10\Lib\10.0.26100.0\um\x64"
export INCLUDE="C:\BuildTools\VC\Tools\MSVC\14.44.35207\include;C:\Program Files (x86)\Windows Kits\10\Include\10.0.26100.0\ucrt;C:\Program Files (x86)\Windows Kits\10\Include\10.0.26100.0\um;C:\Program Files (x86)\Windows Kits\10\Include\10.0.26100.0\shared;C:\Program Files (x86)\Windows Kits\10\Include\10.0.26100.0\winrt"
