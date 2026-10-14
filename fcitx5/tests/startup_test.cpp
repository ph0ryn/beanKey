#include "engine.h"
#include "fixtures.h"

#include <fcitx-utils/capabilityflags.h>
#include <fcitx-utils/event.h>
#include <fcitx-utils/eventloopinterface.h>
#include <fcitx-utils/keysym.h>
#include <fcitx-utils/misc.h>
#include <fcitx/event.h>
#include <fcitx/inputmethodentry.h>
#include <fcitx/inputpanel.h>
#include <fcitx/instance.h>
#include <fcitx/statusarea.h>

#include <poll.h>
#include <sys/socket.h>
#include <sys/un.h>
#include <unistd.h>

#include <array>
#include <chrono>
#include <cstdlib>
#include <cstring>
#include <filesystem>
#include <fstream>
#include <functional>
#include <iostream>
#include <string>
#include <thread>

namespace {

using namespace std::chrono_literals;

bool report(bool condition, const char *message) {
  if (!condition) {
    std::cerr << message << '\n';
  }
  return condition;
}

// Pump the real frontend loop, including dispatcher completions and heartbeats.
bool runUntil(fcitx::Instance &instance, const std::function<bool()> &ready,
              std::chrono::milliseconds timeout) {
  bool completed = false;
  const auto deadline = std::chrono::steady_clock::now() + timeout;
  // Zero accuracy permits 250ms coalescing in sd-event, hiding heartbeat cadence.
  auto timer = instance.eventLoop().addTimeEvent(
      CLOCK_MONOTONIC, fcitx::now(CLOCK_MONOTONIC) + 25000, 1,
      [&](fcitx::EventSourceTime *source, std::uint64_t) {
        completed = ready();
        if (completed || std::chrono::steady_clock::now() >= deadline) {
          instance.eventLoop().exit();
        } else {
          source->setNextInterval(25000);
          source->setOneShot();
        }
        return true;
      });
  instance.eventLoop().exec();
  return completed;
}

std::size_t lineCount(const std::filesystem::path &path) {
  std::ifstream input(path);
  std::size_t count = 0;
  std::string line;
  while (std::getline(input, line) && !input.eof()) {
    ++count;
  }
  return count;
}

bool serveKeys(int connection) {
  timeval timeout{3, 0};
  if (setsockopt(connection, SOL_SOCKET, SO_RCVTIMEO, &timeout,
                 sizeof(timeout)) != 0) {
    return false;
  }
  std::string session;
  for (std::uint64_t id = 1; id <= 3; ++id) {
    beankey::v1::Envelope request;
    if (!request.ParseFromString(beankey::test::readFrame(connection)) ||
        request.protocol_version() != 1 || request.request_id() != id) {
      return false;
    }
    if (id == 1) {
      if (!request.has_start_session() || request.session_id().empty()) {
        return false;
      }
      session = request.session_id();
    } else if (request.session_id() != session ||
               (id == 2 && (!request.has_key_event() ||
                            request.key_event().input() != "a")) ||
               (id == 3 && !request.has_end_session())) {
      return false;
    }
    beankey::v1::Envelope response;
    response.set_protocol_version(1);
    response.set_request_id(id);
    response.set_session_id(session);
    auto *state = response.mutable_state_response();
    state->set_learning_available(true);
    state->set_learning_writable(true);
    state->set_consumed(id == 2);
    state->set_preedit(id == 2 ? "あ" : "");
    state->set_preedit_cursor(id == 2 ? 1 : 0);
    state->set_reset(id != 2);
    std::string payload;
    if (!response.SerializeToString(&payload) ||
        !beankey::test::writeFrame(connection, payload)) {
      return false;
    }
  }
  return true;
}

// The engine starts this executable as a daemon. Observe its actual arguments
// and gate readiness without loading a conversion model. Bound every child wait.
int daemon(int argc, char **argv) {
  if (argc != 7 || std::string(argv[1]) != "--config" ||
      std::string(argv[3]) != "--runtime-root" ||
      std::string(argv[5]) != "--learning-directory") {
    return 1;
  }
  const std::filesystem::path runtime = argv[4];
  {
    std::ofstream output(runtime / "config-path");
    output << argv[2];
    std::ofstream marker(runtime / "launches", std::ios::app);
    marker << "launch\n";
    if (!output || !marker) {
      return 1;
    }
  }
  if (std::filesystem::exists(runtime / "reject-startup")) {
    return 1;
  }
  const bool gated = std::filesystem::exists(runtime / "gated");
  const auto deadline = std::chrono::steady_clock::now() + 2s;
  while (gated && !std::filesystem::exists(runtime / "allow-ready")) {
    if (std::chrono::steady_clock::now() >= deadline) {
      return 1;
    }
    std::this_thread::sleep_for(5ms);
  }
  const std::string path = (runtime / "beankey/daemon.sock").string();
  sockaddr_un address{};
  address.sun_family = AF_UNIX;
  if (path.size() >= sizeof(address.sun_path)) {
    return 1;
  }
  std::memcpy(address.sun_path, path.c_str(), path.size() + 1);
  const int listener = socket(AF_UNIX, SOCK_STREAM | SOCK_CLOEXEC, 0);
  if (listener < 0) {
    return 1;
  }
  if (bind(listener, reinterpret_cast<const sockaddr *>(&address),
           sizeof(address)) != 0 || listen(listener, 1) != 0) {
    close(listener);
    return 1;
  }
  pollfd pending{listener, POLLIN, 0};
  if (poll(&pending, 1, 2000) <= 0) {
    close(listener);
    return 1;
  }
  const int connection = accept(listener, nullptr, nullptr);
  const bool valid = connection >= 0 && (!gated || serveKeys(connection));
  if (connection >= 0) {
    close(connection);
  }
  close(listener);
  return valid ? 0 : 1;
}

bool startup(const std::filesystem::path &root, const std::string &xdgConfig,
             const std::string &expected, const std::string &mode = "") {
  const auto runtime = root / "runtime";
  std::filesystem::create_directories(runtime / "beankey");
  if (!mode.empty()) {
    std::ofstream(runtime / (mode == "failure" ? "reject-startup" : "gated"));
  }
  if (setenv("HOME", (root / "home").c_str(), 1) != 0 ||
      setenv("XDG_CONFIG_HOME", xdgConfig.c_str(), 1) != 0 ||
      setenv("XDG_RUNTIME_DIR", runtime.c_str(), 1) != 0 ||
      setenv("XDG_STATE_HOME", (root / "state").c_str(), 1) != 0) {
    return false;
  }
  char name[] = "beankey-startup-test";
  char disable[] = "--disable=all";
  char *arguments[] = {name, disable};
  fcitx::Instance instance(2, arguments);
  if (!report(instance.initialized(), "Fcitx instance did not initialize")) {
    return false;
  }
  bool valid = true;
  std::chrono::steady_clock::time_point shutdownBegin;
  {
    fcitx::BeanKeyEngine engine(&instance);
    beankey::test::TestInputContext inputContext(instance.inputContextManager());
    inputContext.setCapabilityFlags(fcitx::CapabilityFlag::Preedit);
    fcitx::InputMethodEntry entry("beankey", "beanKey", "ja", "beankey");
    const auto begin = std::chrono::steady_clock::now();
    if (mode.empty()) {
      static_cast<void>(engine.ensureConnected());
    } else {
      fcitx::FocusInEvent focus(&inputContext);
      engine.activate(entry, focus);
    }
    valid = report(std::chrono::steady_clock::now() - begin < 250ms,
                   "activation blocked on daemon preparation") && valid;
    if (!valid) {
      std::ofstream(runtime / "allow-ready");
      return false;
    }
    if (!mode.empty()) {
      for (const auto symbol : {FcitxKey_x, FcitxKey_period}) {
        fcitx::KeyEvent key(&inputContext, fcitx::Key(symbol));
        const auto keyBegin = std::chrono::steady_clock::now();
        engine.keyEvent(entry, key);
        valid = report(std::chrono::steady_clock::now() - keyBegin < 250ms &&
                           !key.accepted() && inputContext.committed().empty() &&
                           inputContext.inputPanel().clientPreedit().empty() &&
                           !inputContext.inputPanel().candidateList(),
                       "pending preparation blocked or consumed a key") && valid;
      }
    }
    if (mode == "failure") {
      valid = report(runUntil(instance, [&] {
                       static_cast<void>(engine.ensureConnected());
                       return lineCount(runtime / "daemon-finished") >= 2;
                     }, 33s) && lineCount(runtime / "launches") == 2 &&
                         std::chrono::steady_clock::now() - begin >= 31s,
                     "failed startup did not enforce deadline and retry backoff") &&
              valid;
    } else if (mode != "cancelled") {
      bool released = mode.empty();
      bool ready = false;
      bool sent = false;
      std::chrono::steady_clock::duration longestGap{};
      auto lastHeartbeat = std::chrono::steady_clock::now();
      int heartbeats = 0;
      valid = report(runUntil(instance, [&] {
                       ++heartbeats;
                       const auto tick = std::chrono::steady_clock::now();
                       if (!ready) {
                         const auto gap = tick - lastHeartbeat;
                         if (gap > longestGap) {
                           longestGap = gap;
                         }
                       }
                       lastHeartbeat = tick;
                       if (!released) {
                         valid = report(!engine.ensureConnected(),
                                        "daemon connected before readiness gate") &&
                                 valid;
                         if (heartbeats >= 4) {
                           std::ofstream(runtime / "allow-ready");
                           released = true;
                         }
                         return false;
                       }
                       if (!ready) {
                         ready = engine.ensureConnected();
                       }
                       if (ready && mode == "gated" && !sent) {
                         fcitx::KeyEvent fresh(&inputContext,
                                               fcitx::Key(FcitxKey_a));
                         engine.keyEvent(entry, fresh);
                         valid = report(
                             fresh.accepted() &&
                                 inputContext.inputPanel().clientPreedit()
                                         .toString() == "あ" &&
                                 inputContext.statusArea()
                                         .actions(fcitx::StatusGroup::InputMethod)
                                         .size() == 1,
                             "fresh input or learning action was lost after preparation") &&
                                 valid;
                         fcitx::InputContextEvent out(
                             &inputContext, fcitx::EventType::InputContextFocusOut);
                         engine.deactivate(entry, out);
                         valid = report(
                             inputContext.statusArea()
                                 .actions(fcitx::StatusGroup::InputMethod).empty(),
                             "learning action remained after session teardown") &&
                                 valid;
                         sent = true;
                       }
                       return ready &&
                              lineCount(runtime / "daemon-finished") > 0;
                     }, 3s),
                     "daemon readiness was not published on the frontend loop") &&
              valid;
      if (!mode.empty() &&
          (heartbeats < 5 || longestGap >= 250ms ||
           lineCount(runtime / "launches") != 1)) {
        std::cerr << "preparation ticks=" << heartbeats << ", longest gap="
                  << std::chrono::duration_cast<std::chrono::milliseconds>(
                         longestGap).count()
                  << "ms, launches=" << lineCount(runtime / "launches") << '\n';
      }
      valid = report(mode.empty() ||
                         (heartbeats >= 5 && longestGap < 250ms &&
                          lineCount(runtime / "launches") == 1),
                     "pending keys relaunched daemon or starved the frontend loop") &&
              valid;
    }
    shutdownBegin = std::chrono::steady_clock::now();
  }
  valid = report(std::chrono::steady_clock::now() - shutdownBegin < 250ms,
                 "frontend shutdown waited for daemon preparation") && valid;
  if (mode == "cancelled") {
    std::ofstream(runtime / "allow-ready");
    valid = report(runUntil(instance, [&] {
                     return lineCount(runtime / "daemon-finished") > 0;
                   }, 3s),
                   "cancelled test daemon outlived its bounded startup") && valid;
  }
  std::ifstream input(runtime / "config-path");
  std::string actual;
  std::getline(input, actual);
  valid = report(input && actual == expected,
                 "daemon received the wrong configuration path") && valid;
  if (mode == "gated") {
    std::ifstream finished(runtime / "daemon-finished");
    int status = 1;
    finished >> status;
    valid = report(finished && status == 0,
                   "preparation replayed pending keys or sent unexpected IPC") &&
            valid;
  }
  return valid;
}

} // namespace

int main(int argc, char **argv) {
  if (argc > 1) {
    const int status = daemon(argc, argv);
    if (argc == 7) {
      std::ofstream(std::filesystem::path(argv[4]) / "daemon-finished",
                    std::ios::app) << status << '\n';
    }
    return status;
  }
  std::array<char, 64> directory{};
  std::strcpy(directory.data(), "/tmp/beankey-startup.XXXXXX");
  if (mkdtemp(directory.data()) == nullptr) {
    return 1;
  }
  const std::filesystem::path root = directory.data();
  bool valid = startup(root / "system", "", BEANKEY_CONFIG_PATH);
  for (const auto &scenario : {"home", "xdg", "relative", "dangling"}) {
    const auto testRoot = root / scenario;
    const bool explicitXdg =
        std::string(scenario) == "xdg" || std::string(scenario) == "dangling";
    const auto configRoot =
        explicitXdg ? testRoot / "config" : testRoot / "home/.config";
    const auto configFile = configRoot / "beankey/config.toml";
    std::filesystem::create_directories(configFile.parent_path());
    if (std::string(scenario) == "dangling") {
      std::filesystem::create_symlink(testRoot / "missing.toml", configFile);
    } else {
      const auto target = testRoot / "generated.toml";
      std::ofstream(target) << "# Home Manager generated configuration\n";
      std::filesystem::create_symlink(target, configFile);
    }
    const std::string xdg = explicitXdg ? configRoot.string()
                             : std::string(scenario) == "relative" ? "relative"
                                                                  : "";
    valid = startup(testRoot, xdg, configFile.string()) && valid;
  }
  valid = startup(root / "gated", "", BEANKEY_CONFIG_PATH, "gated") && valid;
  if (valid) {
    valid = startup(root / "cancelled", "", BEANKEY_CONFIG_PATH, "cancelled") &&
            valid;
    valid = startup(root / "failure", "", BEANKEY_CONFIG_PATH, "failure") && valid;
  }
  std::filesystem::remove_all(root);
  return valid ? 0 : 1;
}
