#!/usr/bin/env bash
set -euo pipefail

mobile_root="$(cd "$(dirname "$0")/.." && pwd)"
output_root="${IOS_OUTPUT_DIR:-$mobile_root/../dist/ios}"
build_root="$(mktemp -d "${TMPDIR:-/tmp}/agent-monitor-ios.XXXXXX")"
trap 'rm -rf "$build_root"' EXIT

cd "$mobile_root"
xcodebuild -version
npm ci
npx cap sync ios

xcodebuild archive \
  -workspace ios/App/App.xcworkspace \
  -scheme App \
  -configuration Release \
  -destination 'generic/platform=iOS' \
  -archivePath "$build_root/App.xcarchive" \
  CODE_SIGNING_ALLOWED=NO \
  CODE_SIGNING_REQUIRED=NO \
  CODE_SIGN_IDENTITY=''

app_path="$build_root/App.xcarchive/Products/Applications/App.app"
python3 - "$app_path" <<'PY'
import json
import plistlib
import sys
from pathlib import Path

app = Path(sys.argv[1])
with (app / "Info.plist").open("rb") as source:
    info = plistlib.load(source)
assert info["CFBundleIdentifier"] == "com.vitahsu.agentmonitor", info
assert "iPhoneOS" in info["CFBundleSupportedPlatforms"], info
assert (app / info["CFBundleExecutable"]).is_file()
config = json.loads((app / "capacitor.config.json").read_text())
assert config["server"]["url"] == "https://monitor.vita-llm.com/portal?v=3", config
print("Verified iOS", info["CFBundleShortVersionString"], "build", info["CFBundleVersion"])
PY

version="$(/usr/libexec/PlistBuddy -c 'Print CFBundleShortVersionString' "$app_path/Info.plist")"
build="$(/usr/libexec/PlistBuddy -c 'Print CFBundleVersion' "$app_path/Info.plist")"
package="AgentMonitor-iOS-$version-$build.ipa"
mkdir -p "$output_root" "$build_root/package/Payload"
ditto "$app_path" "$build_root/package/Payload/App.app"
COPYFILE_DISABLE=1 ditto -c -k --norsrc --keepParent \
  "$build_root/package/Payload" "$output_root/$package"
unzip -t "$output_root/$package"
(
  cd "$output_root"
  shasum -a 256 "$package" > "$package.sha256"
)
echo "Unsigned IPA ready: $output_root/$package"
