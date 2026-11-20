import AppKit
import Metal
import MetalPerformanceShaders
import NotchCore
import QuartzCore

// The insomnia cover's background (APP-1100): the startup's grainy moving field (StartupStudies.swift StartupStudyField,
// GravityDenseStudy.swift DenseGravitySurface, as StartupArrival's ArrivalFrame composes it at rest), fully frosted.
// The startup draws it with a SwiftUI Canvas on the CPU, far too costly for a cover that stays up for hours, and the
// startup is finalized (APP-612), so this is the x5 prototype's Metal port (Field.swift, within 0.6 % of FERB.app's own
// view), kept to the one background the owner chose: the field drawn at a quarter of the display's pixels, blurred
// (ShieldFrost.blurPoints), lifted a little, the film grain sharp on the glass. Opaque: nothing behind the cover is read.

private let shieldShaders = #"""
#include <metal_stdlib>
using namespace metal;

// u: 0 width pt, 1 height pt, 2 time, 3 energy, 4 crest depth, 5 band offset, 6 field scale, 7 settle
struct VOut { float4 pos [[position]]; float4 color; };

static float sm(float v) { float t = clamp(v, 0.0, 1.0); return t * t * (3.0 - 2.0 * t); }

static float2 project(float depth, float angle, constant float* u) {
    float base = min(u[0], u[1]);
    float core = 0.018;
    float radius = base * (core + pow(depth, 1.55) * (0.92 - core)) * u[6];
    float phase3 = -depth * 7.0 + u[2] * 0.65;
    float phase5 = depth * 11.0 - u[2] * 0.4;
    float fold = 1.0 + 0.07 * sin(angle * 3.0 + phase3) + 0.025 * sin(angle * 5.0 + phase5);
    float twist = depth * 1.35 + u[2] * 0.12;
    float x = cos(angle + twist) * radius * fold;
    float y = sin(angle + twist) * radius * fold;
    float incl = 0.88 + 0.12 * sin(u[2] * 0.55);
    float persp = base * 1.8 / (base * 1.8 + y * sin(incl));
    float py = y * cos(incl);
    float roll = -0.22 + sin(u[2] * 0.40) * 0.12;
    float cx = u[0] / 2.0 + sin(depth * 4.0 + u[2] * 0.3) * depth * 24.0 * u[6];
    float cy = u[1] / 2.0 + cos(depth * 3.0 - u[2] * 0.3) * depth * 12.0 * u[6];
    return float2(cx + (x * cos(roll) - py * sin(roll)) * persp, cy + (x * sin(roll) + py * cos(roll)) * persp);
}

static float4 light(float angle, float depth, constant float* u) {
    float lightAngle = -1.1 + u[2] * 0.24;
    float edge = sm(depth / 0.06) * sm((1.0 - depth) / 0.13);
    float crest = exp(-pow((depth - u[4]) * 7.0, 2.0)) * u[3];
    float specular = pow(max(0.0, cos(angle - lightAngle - depth * 1.2)), 7.0);
    float counter = pow(max(0.0, cos(angle - lightAngle - depth * 1.2 + M_PI_F)), 5.0);
    float fold = 0.78 + 0.22 * sin(depth * 17.0 + angle * 2.0 - u[2] * 0.3);
    float fiber = 0.94 + 0.06 * sin(depth * 440.0 + angle * 3.0);
    float alpha = edge * (0.12 + specular * 0.42 + counter * 0.22 + crest * 0.24) * fold * fiber * (1.0 - u[7] * 0.6);
    float warm = max(0.0, sin(angle + u[2] * 0.16));
    return float4(0.73 + warm * 0.25, 0.79 - warm * 0.16, 0.83 - warm * 0.39, clamp(alpha, 0.0, 1.0));
}

vertex VOut fieldVertex(uint vid [[vertex_id]], uint iid [[instance_id]], constant float* u [[buffer(0)]]) {
    uint dir = vid / 2u;
    uint side = vid % 2u;
    float d0 = min(1.0, float(iid) / 256.0 + u[5]);
    float d1 = min(1.0, float(iid + 1u) / 256.0 + u[5]);
    float angle = float(dir) / 144.0 * 2.0 * M_PI_F;
    float2 p = project(side == 0u ? d0 : d1, angle, u);
    uint section = min(dir / 24u, 5u);
    float t = float(dir - section * 24u) / 24.0;
    float4 c = mix(light(float(section) * M_PI_F / 3.0, d0, u), light(float(section + 1u) * M_PI_F / 3.0, d0, u), t);
    VOut o;
    o.pos = float4(p.x / u[0] * 2.0 - 1.0, 1.0 - p.y / u[1] * 2.0, 0.0, 1.0);
    o.color = float4(c.rgb * c.a, c.a);
    return o;
}

fragment float4 fieldFragment(VOut in [[stage_in]]) { return in.color; }

struct QOut { float4 pos [[position]]; };
vertex QOut quadVertex(uint vid [[vertex_id]]) {
    float2 p = float2((vid << 1) & 2, vid & 2);
    QOut o; o.pos = float4(p * 2.0 - 1.0, 0.0, 1.0); return o;
}

static float3 softLight(float3 b, float s) {
    float3 d = select(sqrt(b), ((16.0 * b - 12.0) * b + 4.0) * b, b <= 0.25);
    return s <= 0.5 ? b - (1.0 - 2.0 * s) * b * (1.0 - b) : b + (2.0 * s - 1.0) * (d - b);
}

// c: 0 width pt, 1 height pt, 2 px per pt, 3 lift, 4 grain on the glass
fragment float4 frosted(QOut in [[stage_in]], texture2d<float> blurred [[texture(0)]], texture2d<float> grain [[texture(1)]],
                        constant float* c [[buffer(0)]]) {
    constexpr sampler lin(filter::linear, address::clamp_to_edge);
    float2 pt = in.pos.xy / c[2];
    float2 size = float2(c[0], c[1]);
    float s = grain.read(uint2(pt) % 256u).r;
    // The startup's mask: radial, white -> white 0.9 -> clear between 60 pt and 0.65 x the longer side.
    float r = length(pt - size / 2.0);
    float tm = clamp((r - 60.0) / (max(size.x, size.y) * 0.65 - 60.0), 0.0, 1.0);
    float m = tm < 0.5 ? mix(1.0, 0.9, tm * 2.0) : mix(0.9, 0.0, (tm - 0.5) * 2.0);
    float3 ground = float3(0.018, 0.019, 0.022);
    float4 b = blurred.sample(lin, pt / size) * m;
    float3 frost = b.rgb + ground * (1.0 - b.a);
    float3 glass = frost * (1.0 - c[3]) + c[3];
    glass = mix(glass, softLight(clamp(glass, 0.0, 1.0), s), c[4]);
    return float4(glass, 1.0);
}
"""#

/// What the field needs each frame.
struct ShieldFieldFrame {
    var time: Double
    var energy: Double
    var crest: Double
}

/// The compiled shaders and the grain, made once per process.
final class ShieldFieldPipeline {
    let device: MTLDevice
    let queue: MTLCommandQueue
    let field: MTLRenderPipelineState
    let frosted: MTLRenderPipelineState
    let grain: MTLTexture

    /// nil without a Metal device or when the shaders don't compile: the cover is then plain black, still opaque.
    static let shared: ShieldFieldPipeline? = try? ShieldFieldPipeline()

    private init() throws {
        guard let device = MTLCreateSystemDefaultDevice(), let queue = device.makeCommandQueue() else {
            throw CocoaError(.featureUnsupported)
        }
        self.device = device
        self.queue = queue
        let library = try device.makeLibrary(source: shieldShaders, options: nil)
        let fd = MTLRenderPipelineDescriptor()
        fd.vertexFunction = library.makeFunction(name: "fieldVertex")
        fd.fragmentFunction = library.makeFunction(name: "fieldFragment")
        fd.colorAttachments[0].pixelFormat = .rgba16Float
        let a = fd.colorAttachments[0]!
        a.isBlendingEnabled = true
        a.rgbBlendOperation = .add
        a.alphaBlendOperation = .add
        a.sourceRGBBlendFactor = .one
        a.sourceAlphaBlendFactor = .one
        a.destinationRGBBlendFactor = .oneMinusSourceAlpha
        a.destinationAlphaBlendFactor = .oneMinusSourceAlpha
        field = try device.makeRenderPipelineState(descriptor: fd)
        let cd = MTLRenderPipelineDescriptor()
        cd.vertexFunction = library.makeFunction(name: "quadVertex")
        cd.fragmentFunction = library.makeFunction(name: "frosted")
        cd.colorAttachments[0].pixelFormat = .bgra8Unorm
        frosted = try device.makeRenderPipelineState(descriptor: cd)
        let side = ShieldGrain.side
        let gd = MTLTextureDescriptor.texture2DDescriptor(pixelFormat: .r8Unorm, width: side, height: side, mipmapped: false)
        guard let grain = device.makeTexture(descriptor: gd) else { throw CocoaError(.featureUnsupported) }
        grain.replace(region: MTLRegionMake2D(0, 0, side, side), mipmapLevel: 0, withBytes: ShieldGrain.bytes(),
                      bytesPerRow: side)
        self.grain = grain
    }
}

/// One display's frosted field, drawn into its CAMetalLayer when asked (InsomniaCover's frame clock decides when).
final class ShieldFieldRenderer {
    let pipeline: ShieldFieldPipeline
    let layer: CAMetalLayer
    /// The blur in use: ShieldFrost.blurPoints at this display's scale, in the quarter-size texture's texels.
    let blurSigma: Float
    private let blur: MPSImageGaussianBlur
    private var small: MTLTexture?
    private var blurred: MTLTexture?

    init(pipeline: ShieldFieldPipeline, pixelsPerPoint: CGFloat) {
        self.pipeline = pipeline
        layer = CAMetalLayer()
        layer.device = pipeline.device
        layer.pixelFormat = .bgra8Unorm
        layer.framebufferOnly = true
        layer.isOpaque = true
        layer.contentsScale = pixelsPerPoint
        layer.colorspace = CGColorSpace(name: CGColorSpace.sRGB)
        layer.backgroundColor = NSColor.black.cgColor
        blurSigma = Float(ShieldFrost.blurTexels(pixelsPerPoint: Double(pixelsPerPoint)))
        blur = MPSImageGaussianBlur(device: pipeline.device, sigma: blurSigma)
        blur.edgeMode = .clamp
    }

    private func texture(_ w: Int, _ h: Int, write: Bool) -> MTLTexture? {
        let d = MTLTextureDescriptor.texture2DDescriptor(pixelFormat: .rgba16Float, width: max(1, w), height: max(1, h),
                                                         mipmapped: false)
        d.usage = write ? [.shaderRead, .shaderWrite, .renderTarget] : [.shaderRead, .renderTarget]
        d.storageMode = .private
        return pipeline.device.makeTexture(descriptor: d)
    }

    /// Draws one frame for `size` (points).
    func draw(_ frame: ShieldFieldFrame, size: CGSize) {
        let scale = layer.contentsScale
        let pixels = CGSize(width: size.width * scale, height: size.height * scale)
        if layer.drawableSize != pixels { layer.drawableSize = pixels }
        let w = Int(pixels.width * ShieldFrost.blurTextureScale), h = Int(pixels.height * ShieldFrost.blurTextureScale)
        if small?.width != max(1, w) || small?.height != max(1, h) {
            small = texture(w, h, write: false)
            blurred = texture(w, h, write: true)
        }
        guard let small, let blurred, let drawable = layer.nextDrawable(),
              let commands = pipeline.queue.makeCommandBuffer() else { return }
        // The field at rest, as DenseGravitySurface computes it (progress 0.4: no gather, no settle).
        let travel = frame.time * 0.065 + Self.smooth((0.4 - 0.32) / 0.30) * 0.13
        let offset = (1 - (travel * 256).truncatingRemainder(dividingBy: 1)) / 256
        var u: [Float] = [Float(size.width), Float(size.height), Float(frame.time), Float(frame.energy),
                          Float(frame.crest), Float(offset), Float(0.85 + 0.15 + frame.energy * 0.035), 0]
        let pass = MTLRenderPassDescriptor()
        pass.colorAttachments[0].texture = small
        pass.colorAttachments[0].loadAction = .clear
        pass.colorAttachments[0].clearColor = MTLClearColor(red: 0, green: 0, blue: 0, alpha: 0)
        pass.colorAttachments[0].storeAction = .store
        if let encoder = commands.makeRenderCommandEncoder(descriptor: pass) {
            encoder.setRenderPipelineState(pipeline.field)
            encoder.setVertexBytes(&u, length: u.count * 4, index: 0)
            encoder.drawPrimitives(type: .triangleStrip, vertexStart: 0, vertexCount: 290, instanceCount: 256)
            encoder.endEncoding()
        }
        blur.encode(commandBuffer: commands, sourceTexture: small, destinationTexture: blurred)
        let out = MTLRenderPassDescriptor()
        out.colorAttachments[0].texture = drawable.texture
        out.colorAttachments[0].loadAction = .dontCare
        out.colorAttachments[0].storeAction = .store
        if let encoder = commands.makeRenderCommandEncoder(descriptor: out) {
            encoder.setRenderPipelineState(pipeline.frosted)
            var c: [Float] = [Float(size.width), Float(size.height), Float(scale), Float(ShieldFrost.lift),
                              Float(ShieldFrost.grainOnGlass)]
            encoder.setFragmentBytes(&c, length: c.count * 4, index: 0)
            encoder.setFragmentTexture(blurred, index: 0)
            encoder.setFragmentTexture(pipeline.grain, index: 1)
            encoder.drawPrimitives(type: .triangle, vertexStart: 0, vertexCount: 3)
            encoder.endEncoding()
        }
        commands.present(drawable)
        commands.commit()
    }

    private static func smooth(_ value: Double) -> Double {
        let t = min(1, max(0, value))
        return t * t * (3 - 2 * t)
    }
}
