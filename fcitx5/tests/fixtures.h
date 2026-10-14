#pragma once

#include <fcitx/inputcontext.h>
#include <unistd.h>

#include <array>
#include <cstdint>
#include <string>

namespace beankey::test {

inline bool readAll(int socket, void *data, std::size_t size) {
  auto *current = static_cast<std::uint8_t *>(data);
  while (size > 0) {
    const auto count = read(socket, current, size);
    if (count <= 0) {
      return false;
    }
    current += count;
    size -= static_cast<std::size_t>(count);
  }
  return true;
}

inline bool writeAll(int socket, const void *data, std::size_t size) {
  const auto *current = static_cast<const std::uint8_t *>(data);
  while (size > 0) {
    const auto count = write(socket, current, size);
    if (count <= 0) {
      return false;
    }
    current += count;
    size -= static_cast<std::size_t>(count);
  }
  return true;
}

inline std::string readFrame(int socket) {
  std::uint32_t size = 0;
  for (unsigned int index = 0; index < 5; ++index) {
    std::uint8_t byte = 0;
    if (!readAll(socket, &byte, 1)) {
      return {};
    }
    size |= static_cast<std::uint32_t>(byte & 0x7fU) << (index * 7U);
    if ((byte & 0x80U) == 0) {
      std::string payload(size, '\0');
      return readAll(socket, payload.data(), payload.size()) ? payload
                                                           : std::string{};
    }
  }
  return {};
}

inline bool writeFrame(int socket, const std::string &payload) {
  std::array<std::uint8_t, 5> prefix{};
  auto size = static_cast<std::uint32_t>(payload.size());
  std::size_t count = 0;
  do {
    auto byte = static_cast<std::uint8_t>(size & 0x7fU);
    size >>= 7U;
    if (size != 0) {
      byte |= 0x80U;
    }
    prefix[count++] = byte;
  } while (size != 0);
  return writeAll(socket, prefix.data(), count) &&
         writeAll(socket, payload.data(), payload.size());
}

class TestInputContext final : public fcitx::InputContextV2 {
public:
  explicit TestInputContext(fcitx::InputContextManager &manager)
      : InputContextV2(manager, "beankey-engine-test") {
    created();
  }

  ~TestInputContext() override { destroy(); }

  const char *frontend() const override { return "beankey-test"; }

  const std::string &committed() const { return committed_; }
  std::size_t commitCursor() const { return commitCursor_; }
  bool usedCursorCommit() const { return usedCursorCommit_; }

protected:
  void commitStringImpl(const std::string &text) override {
    committed_ += text;
    commitCursor_ = text.size();
    usedCursorCommit_ = false;
  }
  void commitStringWithCursorImpl(const std::string &text,
                                std::size_t cursor) override {
    committed_ += text;
    commitCursor_ = cursor;
    usedCursorCommit_ = true;
  }
  void deleteSurroundingTextImpl(int, unsigned int) override {}
  void forwardKeyImpl(const fcitx::ForwardKeyEvent &) override {}
  void updatePreeditImpl() override {}

private:
  std::string committed_;
  std::size_t commitCursor_ = 0;
  bool usedCursorCommit_ = false;
};

} // namespace beankey::test
