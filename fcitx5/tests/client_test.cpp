#include "client.h"

#include <sys/socket.h>
#include <sys/un.h>
#include <unistd.h>

#include <array>
#ifdef NDEBUG
#undef NDEBUG
#endif
#include <cassert>
#include <chrono>
#include <cstdint>
#include <cstring>
#include <string>
#include <thread>

namespace {

bool readAll(int socket, void *data, std::size_t size) {
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

bool writeAll(int socket, const void *data, std::size_t size) {
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

std::string readFrame(int socket) {
  std::uint32_t size = 0;
  for (unsigned int index = 0; index < 5; ++index) {
    std::uint8_t byte = 0;
    assert(readAll(socket, &byte, 1));
    size |= static_cast<std::uint32_t>(byte & 0x7fU) << (index * 7U);
    if ((byte & 0x80U) == 0) {
      std::string payload(size, '\0');
      assert(readAll(socket, payload.data(), payload.size()));
      return payload;
    }
  }
  return {};
}

void writeLength(int socket, std::uint32_t size) {
  std::array<std::uint8_t, 5> prefix{};
  std::size_t count = 0;
  do {
    auto byte = static_cast<std::uint8_t>(size & 0x7fU);
    size >>= 7U;
    if (size != 0) {
      byte |= 0x80U;
    }
    prefix[count++] = byte;
  } while (size != 0);
  assert(writeAll(socket, prefix.data(), count));
}

void writeFrame(int socket, const std::string &payload) {
  writeLength(socket, static_cast<std::uint32_t>(payload.size()));
  assert(writeAll(socket, payload.data(), payload.size()));
}

} // namespace

int main() {
  std::array<char, 64> directoryTemplate{};
  std::strcpy(directoryTemplate.data(), "/tmp/beankey-client-test.XXXXXX");
  const char *directory = mkdtemp(directoryTemplate.data());
  assert(directory != nullptr);
  const std::string socketPath = std::string(directory) + "/daemon.sock";

  const int listener = socket(AF_UNIX, SOCK_STREAM | SOCK_CLOEXEC, 0);
  assert(listener >= 0);
  sockaddr_un address{};
  address.sun_family = AF_UNIX;
  std::memcpy(address.sun_path, socketPath.c_str(), socketPath.size() + 1);
  assert(bind(listener, reinterpret_cast<const sockaddr *>(&address),
              sizeof(address)) == 0);
  assert(listen(listener, 1) == 0);

  std::thread server([listener] {
    const int connection = accept4(listener, nullptr, nullptr, SOCK_CLOEXEC);
    assert(connection >= 0);
    beankey::v1::Envelope request;
    assert(request.ParseFromString(readFrame(connection)));
    beankey::v1::Envelope response;
    response.set_protocol_version(request.protocol_version());
    response.set_request_id(request.request_id());
    response.set_session_id(request.session_id());
    auto *state = response.mutable_state_response();
    state->set_consumed(true);
    state->set_preedit("かな");
    std::string payload;
    assert(response.SerializeToString(&payload));
    writeFrame(connection, payload);
    close(connection);
    close(listener);
  });

  beankey::Client client(socketPath);
  bool launched = false;
  assert(client.ensureConnected([&launched] { launched = true; },
                                std::chrono::milliseconds(500)));
  assert(!launched);
  beankey::v1::Envelope request;
  request.set_protocol_version(1);
  request.set_request_id(7);
  request.set_session_id("client-test");
  request.mutable_reset_session();
  const auto response = client.request(request, std::chrono::milliseconds(500));
  assert(response.has_value());
  assert(response->request_id() == 7);
  assert(response->state_response().consumed());
  assert(response->state_response().preedit() == "かな");

  server.join();
  client.disconnect();
  assert(unlink(socketPath.c_str()) == 0);

  const int oversizedListener = socket(AF_UNIX, SOCK_STREAM | SOCK_CLOEXEC, 0);
  assert(oversizedListener >= 0);
  assert(bind(oversizedListener, reinterpret_cast<const sockaddr *>(&address),
              sizeof(address)) == 0);
  assert(listen(oversizedListener, 1) == 0);
  std::thread oversizedServer([oversizedListener] {
    const int connection =
        accept4(oversizedListener, nullptr, nullptr, SOCK_CLOEXEC);
    assert(connection >= 0);
    assert(!readFrame(connection).empty());
    writeLength(connection, static_cast<std::uint32_t>(
                                beankey::Client::kMaximumMessageSize + 1));
    close(connection);
    close(oversizedListener);
  });
  beankey::Client oversized(socketPath);
  assert(oversized.ensureConnected([] {}, std::chrono::milliseconds(500)));
  assert(!oversized.request(request, std::chrono::milliseconds(500)));
  assert(!oversized.connected());
  oversizedServer.join();
  assert(unlink(socketPath.c_str()) == 0);

  const int malformedListener = socket(AF_UNIX, SOCK_STREAM | SOCK_CLOEXEC, 0);
  assert(malformedListener >= 0);
  assert(bind(malformedListener, reinterpret_cast<const sockaddr *>(&address),
              sizeof(address)) == 0);
  assert(listen(malformedListener, 1) == 0);
  std::thread malformedServer([malformedListener] {
    const int connection =
        accept4(malformedListener, nullptr, nullptr, SOCK_CLOEXEC);
    assert(connection >= 0);
    assert(!readFrame(connection).empty());
    writeFrame(connection, std::string(1, static_cast<char>(0xff)));
    close(connection);
    close(malformedListener);
  });
  beankey::Client malformed(socketPath);
  assert(malformed.ensureConnected([] {}, std::chrono::milliseconds(500)));
  assert(!malformed.request(request, std::chrono::milliseconds(500)));
  assert(!malformed.connected());
  malformedServer.join();
  assert(unlink(socketPath.c_str()) == 0);

  assert(rmdir(directory) == 0);

  bool missingLaunch = false;
  beankey::Client missing(socketPath);
  assert(!missing.ensureConnected([&missingLaunch] { missingLaunch = true; },
                                  std::chrono::milliseconds(50)));
  assert(missingLaunch);
  return 0;
}
