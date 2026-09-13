import SwiftUI

/// GitHub 章鱼猫标志（invertocat）的矢量移植，来自 Simple Icons 的 24×24 路径，
/// 以当前前景色填充；避免引入图片资产或第三方依赖。
struct GitHubMark: Shape {
    func path(in rect: CGRect) -> Path {
        var path = Path()
        path.move(to: CGPoint(x: 12, y: 0.297))
        path.addCurve(to: CGPoint(x: 0, y: 12.297), control1: CGPoint(x: 5.37, y: 0.297), control2: CGPoint(x: 0, y: 5.67))
        path.addCurve(to: CGPoint(x: 8.205, y: 23.682), control1: CGPoint(x: 0, y: 17.6), control2: CGPoint(x: 3.438, y: 22.097))
        path.addCurve(to: CGPoint(x: 9.025, y: 23.105), control1: CGPoint(x: 8.805, y: 23.795), control2: CGPoint(x: 9.025, y: 23.424))
        path.addCurve(to: CGPoint(x: 9.01, y: 21.065), control1: CGPoint(x: 9.025, y: 22.82), control2: CGPoint(x: 9.015, y: 22.065))
        path.addCurve(to: CGPoint(x: 4.968, y: 19.455), control1: CGPoint(x: 5.672, y: 21.789), control2: CGPoint(x: 4.968, y: 19.455))
        path.addCurve(to: CGPoint(x: 3.633, y: 17.7), control1: CGPoint(x: 4.422, y: 18.07), control2: CGPoint(x: 3.633, y: 17.7))
        path.addCurve(to: CGPoint(x: 3.717, y: 16.971), control1: CGPoint(x: 2.546, y: 16.956), control2: CGPoint(x: 3.717, y: 16.971))
        path.addCurve(to: CGPoint(x: 5.555, y: 18.207), control1: CGPoint(x: 4.922, y: 17.055), control2: CGPoint(x: 5.555, y: 18.207))
        path.addCurve(to: CGPoint(x: 9.05, y: 19.205), control1: CGPoint(x: 6.625, y: 20.042), control2: CGPoint(x: 8.364, y: 19.512))
        path.addCurve(to: CGPoint(x: 9.81, y: 17.6), control1: CGPoint(x: 9.158, y: 18.429), control2: CGPoint(x: 9.467, y: 17.9))
        path.addCurve(to: CGPoint(x: 4.344, y: 11.67), control1: CGPoint(x: 7.145, y: 17.3), control2: CGPoint(x: 4.344, y: 16.268))
        path.addCurve(to: CGPoint(x: 5.579, y: 8.45), control1: CGPoint(x: 4.344, y: 10.36), control2: CGPoint(x: 4.809, y: 9.29))
        path.addCurve(to: CGPoint(x: 5.684, y: 5.274), control1: CGPoint(x: 5.444, y: 8.147), control2: CGPoint(x: 5.039, y: 6.927))
        path.addCurve(to: CGPoint(x: 8.984, y: 6.504), control1: CGPoint(x: 5.684, y: 5.274), control2: CGPoint(x: 6.689, y: 4.952))
        path.addCurve(to: CGPoint(x: 11.984, y: 6.099), control1: CGPoint(x: 9.944, y: 6.237), control2: CGPoint(x: 10.964, y: 6.105))
        path.addCurve(to: CGPoint(x: 14.984, y: 6.504), control1: CGPoint(x: 13.004, y: 6.105), control2: CGPoint(x: 14.024, y: 6.237))
        path.addCurve(to: CGPoint(x: 18.269, y: 5.274), control1: CGPoint(x: 17.264, y: 4.952), control2: CGPoint(x: 18.269, y: 5.274))
        path.addCurve(to: CGPoint(x: 18.389, y: 8.45), control1: CGPoint(x: 18.914, y: 6.927), control2: CGPoint(x: 18.509, y: 8.147))
        path.addCurve(to: CGPoint(x: 19.619, y: 11.67), control1: CGPoint(x: 19.154, y: 9.29), control2: CGPoint(x: 19.619, y: 10.36))
        path.addCurve(to: CGPoint(x: 14.144, y: 17.59), control1: CGPoint(x: 19.619, y: 16.28), control2: CGPoint(x: 16.814, y: 17.295))
        path.addCurve(to: CGPoint(x: 14.954, y: 19.81), control1: CGPoint(x: 14.564, y: 17.95), control2: CGPoint(x: 14.954, y: 18.686))
        path.addCurve(to: CGPoint(x: 14.939, y: 23.096), control1: CGPoint(x: 14.954, y: 21.416), control2: CGPoint(x: 14.939, y: 22.706))
        path.addCurve(to: CGPoint(x: 15.764, y: 23.666), control1: CGPoint(x: 14.939, y: 23.411), control2: CGPoint(x: 15.149, y: 23.786))
        path.addCurve(to: CGPoint(x: 24, y: 12.297), control1: CGPoint(x: 20.565, y: 22.092), control2: CGPoint(x: 24, y: 17.592))
        path.addCurve(to: CGPoint(x: 12, y: 0.297), control1: CGPoint(x: 24, y: 5.67), control2: CGPoint(x: 18.627, y: 0.297))
        path.closeSubpath()

        // 设计空间为 24×24，按目标矩形等比缩放并居中
        let scale = min(rect.width / 24, rect.height / 24)
        let translation = CGPoint(
            x: (rect.width - 24 * scale) / 2,
            y: (rect.height - 24 * scale) / 2
        )
        return path.applying(
            CGAffineTransform(scaleX: scale, y: scale)
                .concatenating(CGAffineTransform(translationX: translation.x, y: translation.y))
        )
    }
}
