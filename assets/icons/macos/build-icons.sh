#!/bin/bash
# Rebuilds every app icon asset from the Icon Composer document in this folder.
#
# RemCmd.icon is the only source. Apple's own renderer draws it, so the glass,
# shadows, and shape match what macOS shows; the other platforms reuse those renders.
# Requires Xcode 26 or newer (Icon Composer, actool) on macOS.
set -euo pipefail

cd "$(dirname "$0")"
icons_dir="$(cd .. && pwd)"
document="RemCmd.icon"
ictool="$(xcode-select -p)/../Applications/Icon Composer.app/Contents/Executables/ictool"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

render() { # <artwork pixels> <canvas pixels> <output>
    "$ictool" "$document" --export-image --output-file "$work/render.png" --platform macOS \
        --rendition Default --width "$1" --height "$1" --scale 1 >/dev/null
    "$work/compose" "$work/render.png" "$2" "$3"
}

# The renderer writes 16-bit Display P3. This redraws a render as 8-bit sRGB, which
# every platform's icon loader reads, optionally centred on a larger canvas with the
# shadow of the macOS icon template.
cat >"$work/compose.swift" <<'SWIFT'
import AppKit

let arguments = CommandLine.arguments
let canvas = Int(arguments[2])!
let data = try! Data(contentsOf: URL(fileURLWithPath: arguments[1]))
let body = NSBitmapImageRep(data: data)!.cgImage!
let context = CGContext(
    data: nil, width: canvas, height: canvas, bitsPerComponent: 8, bytesPerRow: 0,
    space: CGColorSpace(name: CGColorSpace.sRGB)!,
    bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!
let side = CGFloat(canvas)
let inset = (side - CGFloat(body.width)) / 2
context.interpolationQuality = .high
if inset > 0 {
    context.setShadow(
        offset: CGSize(width: 0, height: -side * 10 / 1024), blur: side * 10 / 1024,
        color: CGColor(red: 0, green: 0, blue: 0, alpha: 0.3))
}
context.draw(body, in: CGRect(x: inset, y: inset, width: CGFloat(body.width), height: CGFloat(body.height)))
let output = NSBitmapImageRep(cgImage: context.makeImage()!)
try! output.representation(using: .png, properties: [:])!.write(to: URL(fileURLWithPath: arguments[3]))
SWIFT
xcrun swiftc -O -o "$work/compose" "$work/compose.swift"

# Full-bleed renders: Linux, Windows, and the icon shown inside the app.
png_sizes=(16 20 24 28 32 36 40 48 56 60 64 72 80 96 128 256 512 1024)
for size in "${png_sizes[@]}"; do
    render "$size" "$size" "$icons_dir/remcmd-$size.png"
done

# Windows icon: PNG-compressed entries, 256px and below.
python3 - "$icons_dir" <<'PY'
import struct, sys
root = sys.argv[1]
sizes = [16, 20, 24, 28, 32, 36, 40, 48, 56, 60, 64, 72, 80, 96, 128, 256]
images = [open(f"{root}/remcmd-{size}.png", "rb").read() for size in sizes]
offset = 6 + 16 * len(sizes)
header = struct.pack("<HHH", 0, 1, len(sizes))
entries = b""
for size, image in zip(sizes, images):
    entries += struct.pack("<BBBBHHII", size % 256, size % 256, 0, 0, 1, 32, len(image), offset)
    offset += len(image)
open(f"{root}/remcmd.ico", "wb").write(header + entries + b"".join(images))
PY

# macOS before 26 reads the .icns. Its template keeps the artwork inside a margin
# (824 of 1024 points) above a soft shadow, so the icon sits level with its neighbours.
iconset="$work/remcmd.iconset"
mkdir "$iconset"
compose() { # <canvas pixels> <iconset name>
    local margin=$(( ($1 * 100 + 512) / 1024 ))
    (( margin < 1 )) && margin=1
    render $(( $1 - margin * 2 )) "$1" "$iconset/$2"
}
compose 16 icon_16x16.png
compose 32 icon_16x16@2x.png
compose 32 icon_32x32.png
compose 64 icon_32x32@2x.png
compose 128 icon_128x128.png
compose 256 icon_128x128@2x.png
compose 256 icon_256x256.png
compose 512 icon_256x256@2x.png
compose 512 icon_512x512.png
compose 1024 icon_512x512@2x.png
iconutil -c icns "$iconset" -o "$icons_dir/remcmd.icns"

# macOS 26 and later read the layered icon from the asset catalog.
xcrun actool "$document" --compile "$work" --platform macosx \
    --minimum-deployment-target 11.0 --app-icon RemCmd \
    --output-partial-info-plist "$work/partial.plist" >/dev/null
cp "$work/Assets.car" Assets.car

echo "Rebuilt icons in $icons_dir"
