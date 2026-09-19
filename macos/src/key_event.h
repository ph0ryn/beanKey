#pragma once
#include "beankey.pb.h"
#import <Cocoa/Cocoa.h>

namespace beankey::macos {
v1::KeyEvent keyEvent(NSEvent *event);
}
