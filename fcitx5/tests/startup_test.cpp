#include "engine.h"

#include <fcitx/instance.h>

#include <sys/socket.h>
#include <sys/un.h>
#include <unistd.h>

#include <array>
#include <cstdlib>
#include <cstring>
#include <filesystem>
#include <fstream>
#include <iostream>
#include <string>

namespace {

// The engine starts this executable as a daemon. Observe its actual arguments
// and accept the transport connection without loading a conversion model.
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
    if (!output) {
      return 1;
    }
  }
  const std::string path = (runtime / "beankey/daemon.sock").string();
  sockaddr_un address{};
  address.sun_family = AF_UNIX;
  if (path.size() >= sizeof(address.sun_path)) {
    return 1;
  }
  std::memcpy(address.sun_path, path.c_str(), path.size() + 1);
  const int listener = socket(AF_UNIX, SOCK_STREAM | SOCK_CLOEXEC, 0);
  if (listener < 0 ||
      bind(listener, reinterpret_cast<const sockaddr *>(&address),
           sizeof(address)) != 0 ||
      listen(listener, 1) != 0) {
    return 1;
  }
  const int connection = accept(listener, nullptr, nullptr);
  if (connection < 0) {
    return 1;
  }
  close(connection);
  close(listener);
  return 0;
}

bool startup(const std::filesystem::path &root, const std::string &xdgConfig,
             const std::string &expected) {
  const auto runtime = root / "runtime";
  std::filesystem::create_directories(runtime / "beankey");
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
  fcitx::BeanKeyEngine engine(&instance);
  if (!instance.initialized() || !engine.ensureConnected()) {
    std::cerr << "daemon startup failed for " << root << '\n';
    return false;
  }
  std::ifstream input(runtime / "config-path");
  std::string actual;
  std::getline(input, actual);
  if (!input || actual != expected) {
    std::cerr << "expected config " << expected << ", received " << actual
              << '\n';
    return false;
  }
  return true;
}

} // namespace

int main(int argc, char **argv) {
  if (argc > 1) {
    return daemon(argc, argv);
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
  std::filesystem::remove_all(root);
  return valid ? 0 : 1;
}
