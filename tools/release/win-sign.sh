#!/bin/bash
# Authenticode-signs raea's own Windows programs before release.sh zips them (#221: Smart App Control and SmartScreen
# block an unsigned raea.exe and its DLL on a PC that has them on). release.sh calls it when RAEA_WIN_SIGN=1, with the
# files; the others in bin\ (Microsoft's, FFmpeg, uv) keep their makers' signatures or none, and are never re-signed.
# The same account and profile as the app's (raea-platform tools/raea-app/win/sign.sh): Azure Artifact Signing, account
# raeasigning, timestamped by Microsoft's TSA so a signature outlives its days-long certificate. The access token comes
# from the Azure CLI, signed in by a person (az login) or by the release workflow's own identity (azure/login).
#   tools/release/win-sign.sh FILE...
set -euo pipefail
[ $# -gt 0 ] || { echo "usage: win-sign.sh FILE..." >&2; exit 2; }
command -v jsign >/dev/null || { echo "win-sign.sh: no jsign (brew install jsign; choco install jsign)" >&2; exit 1; }
endpoint=${RAEA_SIGN_ENDPOINT:-eus.codesigning.azure.net}
account=${RAEA_SIGN_ACCOUNT:-raeasigning}
profile=${RAEA_SIGN_PROFILE:-raea-app}
token=$(az account get-access-token --resource https://codesigning.azure.net --query accessToken -o tsv | tr -d '\r') ||
  { echo "win-sign.sh: no Azure token (az login)" >&2; exit 1; }
jsign --storetype TRUSTEDSIGNING --keystore "$endpoint" --storepass "$token" --alias "$account/$profile" \
  --tsaurl http://timestamp.acs.microsoft.com --tsmode RFC3161 --replace --name raea --url https://raea.io "$@"
