#!/usr/bin/env swift
// 生成 macOS 应用图标（1024×1024 PNG）。
//
// 设计：品牌蓝 #00A1D6 → 品牌粉 #FB749A 的对角渐变，
// 中间一枚圆角白色播放三角。与 App 内主题色一致。
//
// 用法：swift scripts/make-macos-icon.swift <输出.png>

import AppKit
import CoreGraphics
import Foundation

let output = CommandLine.arguments.count > 1
    ? CommandLine.arguments[1]
    : "AppIcon-1024.png"

let size = 1024
guard let rep = NSBitmapImageRep(
    bitmapDataPlanes: nil,
    pixelsWide: size,
    pixelsHigh: size,
    bitsPerSample: 8,
    samplesPerPixel: 4,
    hasAlpha: true,
    isPlanar: false,
    colorSpaceName: .deviceRGB,
    bytesPerRow: 0,
    bitsPerPixel: 0
) else {
    fatalError("无法创建位图")
}

guard let graphics = NSGraphicsContext(bitmapImageRep: rep) else {
    fatalError("无法创建绘图上下文")
}
NSGraphicsContext.saveGraphicsState()
NSGraphicsContext.current = graphics
let ctx = graphics.cgContext
ctx.setShouldAntialias(true)
ctx.interpolationQuality = .high

// macOS Big Sur 之后的图标网格：824×824 内容区，四周留 100。
let rect = CGRect(x: 100, y: 100, width: 824, height: 824)
let radius: CGFloat = 185
let shape = CGPath(roundedRect: rect, cornerWidth: radius, cornerHeight: radius, transform: nil)

// 背景渐变。
ctx.saveGState()
ctx.addPath(shape)
ctx.clip()
let colors = [
    CGColor(red: 0x00 / 255.0, green: 0xA1 / 255.0, blue: 0xD6 / 255.0, alpha: 1.0),
    CGColor(red: 0x7B / 255.0, green: 0x7C / 255.0, blue: 0xF0 / 255.0, alpha: 1.0),
    CGColor(red: 0xFB / 255.0, green: 0x74 / 255.0, blue: 0x9A / 255.0, alpha: 1.0),
] as CFArray
let gradient = CGGradient(
    colorsSpace: CGColorSpaceCreateDeviceRGB(),
    colors: colors,
    locations: [0.0, 0.55, 1.0]
)!
ctx.drawLinearGradient(
    gradient,
    start: CGPoint(x: 140, y: 900),
    end: CGPoint(x: 900, y: 120),
    options: []
)
ctx.restoreGState()

// 顶部高光：一层很淡的内描边，避免大色块显得平。
ctx.saveGState()
ctx.addPath(CGPath(
    roundedRect: rect.insetBy(dx: 3, dy: 3),
    cornerWidth: radius - 3,
    cornerHeight: radius - 3,
    transform: nil
))
ctx.setStrokeColor(CGColor(red: 1, green: 1, blue: 1, alpha: 0.18))
ctx.setLineWidth(6)
ctx.strokePath()
ctx.restoreGState()

// 播放三角（圆角），视觉中心略向左修正。
let a = CGPoint(x: 408, y: 716)
let b = CGPoint(x: 408, y: 308)
let c = CGPoint(x: 772, y: 512)
let corner: CGFloat = 42
let tri = CGMutablePath()
tri.move(to: CGPoint(x: (a.x + b.x) / 2, y: (a.y + b.y) / 2))
tri.addArc(tangent1End: b, tangent2End: c, radius: corner)
tri.addArc(tangent1End: c, tangent2End: a, radius: corner)
tri.addArc(tangent1End: a, tangent2End: b, radius: corner)
tri.closeSubpath()

ctx.saveGState()
ctx.setShadow(
    offset: CGSize(width: 0, height: -12),
    blur: 34,
    color: CGColor(red: 0, green: 0, blue: 0, alpha: 0.22)
)
ctx.addPath(tri)
ctx.setFillColor(CGColor(red: 1, green: 1, blue: 1, alpha: 1))
ctx.fillPath()
ctx.restoreGState()

NSGraphicsContext.restoreGraphicsState()

guard let png = rep.representation(using: .png, properties: [:]) else {
    fatalError("PNG 编码失败")
}
try png.write(to: URL(fileURLWithPath: output))
print("已生成 \(output)")
