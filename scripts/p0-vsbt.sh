#!/usr/bin/env bash
set -x
cd /c/Users/Administrator/tools 2>/dev/null || { mkdir -p /c/Users/Administrator/tools; cd /c/Users/Administrator/tools; }
curl -fL -o vs_BuildTools.exe https://aka.ms/vs/17/release/vs_BuildTools.exe
./vs_BuildTools.exe --quiet --wait --norestart --nocache \
  --installPath "C:\BuildTools" \
  --add Microsoft.VisualStudio.Workload.VCTools \
  --add Microsoft.VisualStudio.Component.Windows11SDK.22621 \
  --includeRecommended
echo "VSBT exit=$?"
ls "/c/BuildTools/VC/Tools/MSVC" 2>/dev/null && echo "MSVC present"
