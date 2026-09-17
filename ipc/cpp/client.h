#pragma once

#include <chrono>
#include <cstddef>
#include <functional>
#include <optional>
#include <string>

#include "beankey.pb.h"

namespace beankey {

class Client final {
public:
  static constexpr std::size_t kMaximumMessageSize = 1024 * 1024;

  explicit Client(std::string socketPath);
  ~Client();

  Client(const Client &) = delete;
  Client &operator=(const Client &) = delete;

  bool ensureConnected(const std::function<void()> &launchDaemon,
                       std::chrono::milliseconds startupTimeout);
  std::optional<v1::Envelope> request(const v1::Envelope &request,
                                      std::chrono::milliseconds timeout);
  void disconnect();
  bool connected() const;

private:
  bool connectOnce(std::chrono::steady_clock::time_point deadline);
  bool writeAll(const void *data, std::size_t size,
                std::chrono::steady_clock::time_point deadline);
  bool readAll(void *data, std::size_t size,
               std::chrono::steady_clock::time_point deadline);
  bool wait(short events, std::chrono::steady_clock::time_point deadline);

  std::string socketPath_;
  int socket_ = -1;
};

} // namespace beankey
