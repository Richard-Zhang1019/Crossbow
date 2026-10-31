import AppKit
let path=CommandLine.arguments[1]
let sourceImage=path.hasSuffix(".app") ? NSWorkspace.shared.icon(forFile:path) : NSImage(contentsOfFile:path)!
var image=sourceImage
if CommandLine.arguments.contains("--runtime") {
 // Exercises the same AppKit entry point as macos_icon.rs in this helper only.
 let app=NSApplication.shared
 app.setActivationPolicy(.prohibited)
 let data=try! Data(contentsOf:URL(fileURLWithPath:path))
 app.applicationIconImage=NSImage(data:data)!
 image=app.applicationIconImage
}
print("NSImage",image.size,image.representations.count)
for rep in image.representations {
 print(type(of:rep),rep.pixelsWide,rep.pixelsHigh,(rep as? NSBitmapImageRep)?.colorSpace.localizedName ?? "unknown")
}
for n in [32,64,96,128] {
 let bitmap=CGContext(data:nil,width:n,height:n,bitsPerComponent:8,bytesPerRow:n*4,space:CGColorSpace(name:CGColorSpace.sRGB)!,bitmapInfo:CGImageAlphaInfo.premultipliedLast.rawValue)!
 let ctx=NSGraphicsContext(cgContext:bitmap,flipped:false)
 NSGraphicsContext.saveGraphicsState();NSGraphicsContext.current=ctx
 image.draw(in:NSRect(x:0,y:0,width:n,height:n),from:.zero,operation:.copy,fraction:1)
 ctx.flushGraphics();NSGraphicsContext.restoreGraphicsState()
 var colors:[String:Int]=[:]
 for y in 0..<n {for x in 0..<n {
  let data=bitmap.data!.assumingMemoryBound(to:UInt8.self);let p=(y*n+x)*4
  let r=Int(data[p]),g=Int(data[p+1]),b=Int(data[p+2])
  if b>r+50 && b>g+30 {let key="\(r),\(g),\(b)";colors[key,default:0]+=1}
 }}
 let ranked=colors.sorted{$0.value>$1.value}
 print("draw",n,ranked.prefix(3))
 if CommandLine.arguments.contains("--assert-brand") && n>=64 {
  precondition(ranked.first?.key=="63,82,207", "Native render changed the approved sRGB brand colour")
 }
}
