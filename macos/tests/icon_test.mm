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
  }
}
