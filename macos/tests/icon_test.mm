#import <AppKit/AppKit.h>
#include <cstdlib>
#include <iostream>

static void require(bool value, const char *message) {
  if (!value) {
    std::cerr << message << '\n';
    std::exit(1);
  }
}

int main() {
  @autoreleasepool {
    NSString *path = [NSString stringWithUTF8String:BEANKEY_ICON_PATH];
    NSImage *image = [[NSImage alloc] initWithContentsOfFile:path];
    require(image != nil, "The generated input source icon must be readable");
    require(NSEqualSizes(image.size, NSMakeSize(16, 16)),
            "The input source icon must occupy 16 by 16 points");

    bool hasRetinaRepresentation = false;
    for (NSImageRep *representation in image.representations) {
      if (representation.pixelsWide == 32 && representation.pixelsHigh == 32 &&
          NSEqualSizes(representation.size, NSMakeSize(16, 16))) {
        hasRetinaRepresentation = true;
        break;
      }
    }
    require(hasRetinaRepresentation,
            "The input source icon must include a 32-pixel Retina image");

    NSBitmapImageRep *bitmap = [[NSBitmapImageRep alloc]
        initWithData:[NSData dataWithContentsOfFile:path]];
    require(bitmap != nil, "The icon bitmap must be readable");
    double inkAlpha = 0;
    double weightedY = 0;
    for (NSInteger y = 0; y < bitmap.pixelsHigh; ++y) {
      for (NSInteger x = 0; x < bitmap.pixelsWide; ++x) {
        NSColor *pixel = [[bitmap colorAtX:x y:y]
            colorUsingColorSpace:NSColorSpace.deviceRGBColorSpace];
        const double weight =
            pixel.redComponent < 0.2 ? pixel.alphaComponent : 0;
        inkAlpha += weight;
        weightedY += y * weight;
      }
    }
    require(inkAlpha > 0 && weightedY / inkAlpha > 16.2,
            "The bean glyph must sit slightly below the bitmap center");

    NSColor *stroke = [[bitmap colorAtX:16 y:7]
        colorUsingColorSpace:NSColorSpace.deviceRGBColorSpace];
    NSColor *backing = [[bitmap colorAtX:3 y:16]
        colorUsingColorSpace:NSColorSpace.deviceRGBColorSpace];
    require(stroke.redComponent < 0.2 && stroke.alphaComponent > 0.9,
            "The bean strokes must form a black template image");
    require(backing.alphaComponent < 0.1,
            "The template icon must have a transparent background");

    NSDictionary *info = [NSDictionary
        dictionaryWithContentsOfFile:
            [NSString stringWithUTF8String:BEANKEY_INFO_PLIST_PATH]];
    NSDictionary *mode = info[@"ComponentInputModeDict"][@"tsInputModeListKey"]
                             [@"com.apple.inputmethod.Japanese"];
    require([mode[@"TISIconIsTemplate"] isEqual:@YES],
            "The selected input mode must use template rendering");
    require([mode[@"tsInputModeMenuIconFileKey"]
                isEqualToString:@"beanKey-character.tiff"],
            "The selected input mode must use the bean icon");
  }
}
