#include <CoreGraphics/CoreGraphics.h>
#include <ImageIO/ImageIO.h>
#include <cstring>

int main(int argc, const char *argv[]) {
  if (argc != 2)
    return 2;
  auto color = CGColorSpaceCreateDeviceRGB();
  auto context = CGBitmapContextCreate(nullptr, 32, 32, 8, 0, color,
                                       kCGImageAlphaPremultipliedLast);
  CGColorSpaceRelease(color);
  if (!context)
    return 1;
  CGContextSetRGBFillColor(context, 0, 0, 0, 1);
  CGContextMoveToPoint(context, 7, 6);
  CGContextAddCurveToPoint(context, 0, 13, 5, 28, 16, 29);
  CGContextAddCurveToPoint(context, 28, 31, 34, 18, 25, 13);
  CGContextAddCurveToPoint(context, 21, 10, 22, 2, 15, 2);
  CGContextAddCurveToPoint(context, 11, 2, 9, 3, 7, 6);
  CGContextFillPath(context);
  CGContextSetBlendMode(context, kCGBlendModeClear);
  CGContextSetLineWidth(context, 2);
  CGContextMoveToPoint(context, 10, 7);
  CGContextAddCurveToPoint(context, 20, 11, 9, 20, 23, 25);
  CGContextStrokePath(context);
  auto image = CGBitmapContextCreateImage(context);
  auto url = CFURLCreateFromFileSystemRepresentation(
      nullptr, reinterpret_cast<const UInt8 *>(argv[1]), std::strlen(argv[1]),
      false);
  auto destination =
      CGImageDestinationCreateWithURL(url, CFSTR("public.tiff"), 1, nullptr);
  if (destination)
    CGImageDestinationAddImage(destination, image, nullptr);
  const bool success = destination && CGImageDestinationFinalize(destination);
  if (destination)
    CFRelease(destination);
  CFRelease(url);
  CGImageRelease(image);
  CGContextRelease(context);
  return success ? 0 : 1;
}
