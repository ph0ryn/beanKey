#include "client.h"

#include <sys/poll.h>
#include <sys/socket.h>
#include <sys/un.h>
#include <unistd.h>

#include <algorithm>
#include <array>
#include <cerrno>
#include <chrono>
#include <climits>
#include <cstdint>
#include <cstring>
#include <thread>
#include <utility>

namespace beankey {
namespace {

std::array<std::uint8_t, 5> encodeLength(std::size_t size,
                                         std::size_t &encodedSize) {
  std::array<std::uint8_t, 5> encoded{};
  auto value = static_cast<std::uint32_t>(size);
  encodedSize = 0;
  do {
    auto byte = static_cast<std::uint8_t>(value & 0x7fU);
    value >>= 7U;
    if (value != 0) {
      byte |= 0x80U;
    }
    encoded[encodedSize++] = byte;
  } while (value != 0);
  return encoded;
}

} // namespace

Client::Client(std::string socketPath) : socketPath_(std::move(socketPath)) {}

Client::~Client() { disconnect(); }

bool Client::ensureConnected(const std::function<void()> &launchDaemon,
                             std::chrono::milliseconds startupTimeout) {
  if (connected()) {
    return true;
  }
  const auto deadline = std::chrono::steady_clock::now() + startupTimeout;
  if (connectOnce(deadline)) {
    return true;
  }
  launchDaemon();
  while (std::chrono::steady_clock::now() < deadline) {
    if (connectOnce(deadline)) {
      return true;
    }
    std::this_thread::sleep_for(std::chrono::milliseconds(25));
  }
  return false;
}

std::optional<v1::Envelope> Client::request(const v1::Envelope &request,
                                            std::chrono::milliseconds timeout) {
  if (!connected()) {
    return std::nullopt;
  }
  std::string payload;
  if (!request.SerializeToString(&payload) ||
      payload.size() > kMaximumMessageSize) {
    disconnect();
    return std::nullopt;
  }
  const auto deadline = std::chrono::steady_clock::now() + timeout;
  std::size_t prefixSize = 0;
  const auto prefix = encodeLength(payload.size(), prefixSize);
  if (!writeAll(prefix.data(), prefixSize, deadline) ||
      !writeAll(payload.data(), payload.size(), deadline)) {
    disconnect();
    return std::nullopt;
  }

  std::uint32_t responseSize = 0;
  for (unsigned int index = 0; index < 5; ++index) {
    std::uint8_t byte = 0;
    if (!readAll(&byte, 1, deadline)) {
      disconnect();
      return std::nullopt;
    }
    if (index == 4 && (byte & 0xf0U) != 0) {
      disconnect();
      return std::nullopt;
    }
    responseSize |= static_cast<std::uint32_t>(byte & 0x7fU) << (index * 7U);
    if ((byte & 0x80U) == 0) {
      if (responseSize > kMaximumMessageSize) {
        disconnect();
        return std::nullopt;
      }
      std::string responsePayload(responseSize, '\0');
      if (!readAll(responsePayload.data(), responsePayload.size(), deadline)) {
        disconnect();
        return std::nullopt;
      }
      v1::Envelope response;
      if (!response.ParseFromString(responsePayload)) {
        disconnect();
        return std::nullopt;
      }
      return response;
    }
  }
  disconnect();
  return std::nullopt;
}

void Client::disconnect() {
  if (socket_ >= 0) {
    close(socket_);
    socket_ = -1;
  }
}

bool Client::connected() const { return socket_ >= 0; }

bool Client::connectOnce(std::chrono::steady_clock::time_point deadline) {
  disconnect();
  sockaddr_un address{};
  if (socketPath_.empty() || socketPath_.size() >= sizeof(address.sun_path)) {
    return false;
  }
  socket_ = socket(AF_UNIX, SOCK_STREAM | SOCK_CLOEXEC | SOCK_NONBLOCK, 0);
  if (socket_ < 0) {
    return false;
  }
  address.sun_family = AF_UNIX;
  std::memcpy(address.sun_path, socketPath_.c_str(), socketPath_.size() + 1);
  if (connect(socket_, reinterpret_cast<const sockaddr *>(&address),
              sizeof(address)) == 0) {
    return true;
  }
  if (errno != EINPROGRESS || !wait(POLLOUT, deadline)) {
    disconnect();
    return false;
  }
  int error = 0;
  socklen_t size = sizeof(error);
  if (getsockopt(socket_, SOL_SOCKET, SO_ERROR, &error, &size) != 0 ||
      error != 0) {
    disconnect();
    return false;
  }
  return true;
}

bool Client::writeAll(const void *data, std::size_t size,
                      std::chrono::steady_clock::time_point deadline) {
  const auto *current = static_cast<const std::uint8_t *>(data);
  while (size > 0) {
    const auto written = send(socket_, current, size, MSG_NOSIGNAL);
    if (written > 0) {
      current += written;
      size -= static_cast<std::size_t>(written);
      continue;
    }
    if (written < 0 && errno == EINTR) {
      continue;
    }
    if (written < 0 && (errno == EAGAIN || errno == EWOULDBLOCK) &&
        wait(POLLOUT, deadline)) {
      continue;
    }
    return false;
  }
  return true;
}

bool Client::readAll(void *data, std::size_t size,
                     std::chrono::steady_clock::time_point deadline) {
  auto *current = static_cast<std::uint8_t *>(data);
  while (size > 0) {
    const auto readSize = recv(socket_, current, size, 0);
    if (readSize > 0) {
      current += readSize;
      size -= static_cast<std::size_t>(readSize);
      continue;
    }
    if (readSize < 0 && errno == EINTR) {
      continue;
    }
    if (readSize < 0 && (errno == EAGAIN || errno == EWOULDBLOCK) &&
        wait(POLLIN, deadline)) {
      continue;
    }
    return false;
  }
  return true;
}

bool Client::wait(short events,
                  std::chrono::steady_clock::time_point deadline) {
  while (true) {
    const auto remaining =
        std::chrono::duration_cast<std::chrono::milliseconds>(
            deadline - std::chrono::steady_clock::now());
    if (remaining.count() <= 0) {
      return false;
    }
    pollfd descriptor{socket_, events, 0};
    const auto timeout =
        static_cast<int>(std::min<std::int64_t>(remaining.count(), INT_MAX));
    const int result = poll(&descriptor, 1, timeout);
    if (result > 0) {
      if ((descriptor.revents & events) != 0) {
        return true;
      }
      return false;
    }
    if (result < 0 && errno == EINTR) {
      continue;
    }
    return false;
  }
}

} // namespace beankey
