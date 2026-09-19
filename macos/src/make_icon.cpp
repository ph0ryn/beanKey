#include <CoreGraphics/CoreGraphics.h>
#include <CoreText/CoreText.h>
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
  auto systemFont = CTFontCreateUIFontForLanguage(kCTFontUIFontEmphasizedSystem,
                                                  28, CFSTR("ja"));
  if (!systemFont) {
    CGContextRelease(context);
    return 1;
  }
  auto font = CTFontCreateForString(systemFont, CFSTR("豆"), CFRangeMake(0, 1));
  CFRelease(systemFont);
  const UniChar character = 0x8C46; // 豆
  CGGlyph glyph;
  if (!font || !CTFontGetGlyphsForCharacters(font, &character, &glyph, 1)) {
    if (font)
      CFRelease(font);
    CGContextRelease(context);
    return 1;
  }
  const auto bounds = CTFontGetBoundingRectsForGlyphs(
      font, kCTFontOrientationHorizontal, &glyph, nullptr, 1);
  const CGPoint position = {16 - CGRectGetMidX(bounds),
                            16 - CGRectGetMidY(bounds)};
  CGContextSetRGBFillColor(context, 0, 0, 0, 1);
  CTFontDrawGlyphs(font, &glyph, &position, 1, context);
  CFRelease(font);
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
