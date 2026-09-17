cmake_minimum_required(VERSION 3.20)

file(READ "${CMAKE_CURRENT_SOURCE_DIR}/../Cargo.toml" BEANKEY_CARGO_TOML)
string(REGEX MATCH "version = \"([0-9]+\\.[0-9]+\\.[0-9]+)\"" _ "${BEANKEY_CARGO_TOML}")
if(NOT CMAKE_MATCH_1)
  message(FATAL_ERROR "Could not read the workspace version from Cargo.toml")
endif()

project(beankey-fcitx5 VERSION "${CMAKE_MATCH_1}" LANGUAGES CXX)

include(GNUInstallDirs)
include(CTest)

find_package(Fcitx5Core 5.1.21 REQUIRED)
find_package(Threads REQUIRED)
add_subdirectory(../ipc/cpp ${CMAKE_CURRENT_BINARY_DIR}/ipc)

set(BEANKEY_DAEMON_PATH "beankey-daemon" CACHE FILEPATH "Absolute beanKey daemon path")
set(BEANKEY_CONFIG_PATH "/etc/beankey/config.toml" CACHE FILEPATH "Internal daemon configuration path")
set(BEANKEY_REQUEST_TIMEOUT_MS "5000" CACHE STRING "Synchronous daemon request deadline")

add_library(beankey-key-event STATIC
  src/key_event.cpp
)
set_target_properties(beankey-key-event PROPERTIES POSITION_INDEPENDENT_CODE ON)
target_compile_features(beankey-key-event PUBLIC cxx_std_20)
target_include_directories(beankey-key-event PUBLIC src)
target_compile_options(beankey-key-event PRIVATE -Wall -Wextra -Wpedantic -Werror)
target_link_libraries(beankey-key-event PUBLIC beankey-ipc Fcitx5::Core)

add_library(beankey-engine STATIC
  src/engine.cpp
)
set_target_properties(beankey-engine PROPERTIES POSITION_INDEPENDENT_CODE ON)
target_compile_features(beankey-engine PUBLIC cxx_std_20)
target_compile_options(beankey-engine PRIVATE -Wall -Wextra -Wpedantic -Werror)
target_compile_definitions(beankey-engine PRIVATE
  BEANKEY_DAEMON_PATH="${BEANKEY_DAEMON_PATH}"
  BEANKEY_CONFIG_PATH="${BEANKEY_CONFIG_PATH}"
  BEANKEY_REQUEST_TIMEOUT_MS=${BEANKEY_REQUEST_TIMEOUT_MS}
)
target_link_libraries(beankey-engine PUBLIC beankey-key-event Fcitx5::Core)

add_library(fcitx5-beankey MODULE
  src/factory.cpp
)
set_target_properties(fcitx5-beankey PROPERTIES PREFIX "")
target_compile_features(fcitx5-beankey PRIVATE cxx_std_20)
target_compile_options(fcitx5-beankey PRIVATE -Wall -Wextra -Wpedantic -Werror)
target_link_libraries(fcitx5-beankey PRIVATE beankey-engine)

install(TARGETS fcitx5-beankey DESTINATION ${CMAKE_INSTALL_LIBDIR}/fcitx5)
install(FILES beankey-addon.conf DESTINATION ${CMAKE_INSTALL_DATADIR}/fcitx5/addon RENAME beankey.conf)
install(FILES beankey-inputmethod.conf DESTINATION ${CMAKE_INSTALL_DATADIR}/fcitx5/inputmethod RENAME beankey.conf)
install(FILES themes/beanKey/theme.conf DESTINATION ${CMAKE_INSTALL_DATADIR}/fcitx5/themes/beanKey)

if(BUILD_TESTING)
  add_executable(beankey-key-event-test tests/key_event_test.cpp)
  target_compile_features(beankey-key-event-test PRIVATE cxx_std_20)
  target_compile_options(beankey-key-event-test PRIVATE -Wall -Wextra -Wpedantic -Werror)
  target_link_libraries(beankey-key-event-test PRIVATE beankey-key-event)
  add_test(NAME beankey-key-event COMMAND beankey-key-event-test)

  add_executable(beankey-engine-test tests/engine_test.cpp)
  target_compile_features(beankey-engine-test PRIVATE cxx_std_20)
  target_compile_options(beankey-engine-test PRIVATE -Wall -Wextra -Wpedantic -Werror)
  target_link_libraries(beankey-engine-test PRIVATE beankey-engine Threads::Threads)
  add_test(NAME beankey-engine COMMAND beankey-engine-test)
endif()
