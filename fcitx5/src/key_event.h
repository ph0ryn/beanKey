#pragma once

namespace fcitx {
class KeyEvent;
}

namespace beankey {
namespace v1 {
class KeyEvent;
}

void populateKeyEvent(const fcitx::KeyEvent &source, v1::KeyEvent *target);

} // namespace beankey
