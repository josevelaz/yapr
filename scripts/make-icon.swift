// Draws assets/icon.png (512x512): a microphone symbol on a rounded gradient tile.
// Usage: swift scripts/make-icon.swift assets/icon.png
import AppKit

let output = CommandLine.arguments.dropFirst().first ?? "assets/icon.png"
let size = 512.0
let image = NSImage(size: NSSize(width: size, height: size))
image.lockFocus()

let tile = NSBezierPath(roundedRect: NSRect(x: 0, y: 0, width: size, height: size), xRadius: 112, yRadius: 112)
NSGradient(
    starting: NSColor(red: 0.98, green: 0.36, blue: 0.29, alpha: 1),
    ending: NSColor(red: 0.85, green: 0.15, blue: 0.45, alpha: 1)
)!.draw(in: tile, angle: -90)

let config = NSImage.SymbolConfiguration(pointSize: 280, weight: .semibold)
    .applying(.init(paletteColors: [.white]))
if let symbol = NSImage(systemSymbolName: "mic.fill", accessibilityDescription: nil)?
    .withSymbolConfiguration(config) {
    let rect = NSRect(
        x: (size - symbol.size.width) / 2, y: (size - symbol.size.height) / 2,
        width: symbol.size.width, height: symbol.size.height)
    symbol.draw(in: rect)
}
image.unlockFocus()

let bitmap = NSBitmapImageRep(data: image.tiffRepresentation!)!
try! bitmap.representation(using: .png, properties: [:])!.write(to: URL(fileURLWithPath: output))
print("wrote \(output)")
