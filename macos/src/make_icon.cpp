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
  // Lower the visual mass slightly within the 16-point icon.
  CGContextTranslateCTM(context, 0, 30.5);
  CGContextScaleCTM(context, 1, -1);
  CGContextSetRGBStrokeColor(context, 0, 0, 0, 1);
  CGContextSetLineWidth(context, 2.5);
  CGContextSetLineCap(context, kCGLineCapButt);
  CGContextSetLineJoin(context, kCGLineJoinMiter);
  CGContextMoveToPoint(context, 6, 6.5);
  CGContextAddLineToPoint(context, 26, 6.5);
  CGContextMoveToPoint(context, 9, 11.5);
  CGContextAddLineToPoint(context, 23, 11.5);
  CGContextAddLineToPoint(context, 23, 18.5);
  CGContextAddLineToPoint(context, 9, 18.5);
  CGContextClosePath(context);
  CGContextMoveToPoint(context, 11.5, 21);
  CGContextAddLineToPoint(context, 13.5, 24);
  CGContextMoveToPoint(context, 20.5, 21);
  CGContextAddLineToPoint(context, 18.5, 24);
  CGContextMoveToPoint(context, 5, 27);
  CGContextAddLineToPoint(context, 27, 27);
  CGContextStrokePath(context);
  auto image = CGBitmapContextCreateImage(context);
  auto url = CFURLCreateFromFileSystemRepresentation(
      nullptr, reinterpret_cast<const UInt8 *>(argv[1]), std::strlen(argv[1]),
      false);
  auto destination =
      CGImageDestinationCreateWithURL(url, CFSTR("public.tiff"), 1, nullptr);
  // Keep the 32-pixel Retina image while exposing a 16-point menu icon.
  const double dpi = 144;
  auto resolution = CFNumberCreate(nullptr, kCFNumberDoubleType, &dpi);
  const void *propertyKeys[] = {kCGImagePropertyDPIWidth,
                                kCGImagePropertyDPIHeight};
  const void *propertyValues[] = {resolution, resolution};
  auto properties =
      resolution ? CFDictionaryCreate(nullptr, propertyKeys, propertyValues, 2,
                                      &kCFTypeDictionaryKeyCallBacks,
                                      &kCFTypeDictionaryValueCallBacks)
                 : nullptr;
  if (destination && properties)
    CGImageDestinationAddImage(destination, image, properties);
  const bool success =
      destination && properties && CGImageDestinationFinalize(destination);
  if (properties)
    CFRelease(properties);
  if (resolution)
    CFRelease(resolution);
  if (destination)
    CFRelease(destination);
  CFRelease(url);
  CGImageRelease(image);
  CGContextRelease(context);
  return success ? 0 : 1;
}
