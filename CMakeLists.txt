cmake_minimum_required(VERSION 3.20)
project(beankey LANGUAGES CXX)
include(CTest)

if(NOT CMAKE_BUILD_TYPE AND NOT CMAKE_CONFIGURATION_TYPES)
  set(CMAKE_BUILD_TYPE Release CACHE STRING "Build type" FORCE)
endif()
if(NOT APPLE AND NOT CMAKE_SYSTEM_NAME STREQUAL "Linux")
  message(FATAL_ERROR "beanKey requires Linux or macOS")
endif()
find_program(CARGO_EXECUTABLE cargo REQUIRED)

set(BEANKEY_MODEL "$ENV{BEANKEY_TEST_MODEL}" CACHE FILEPATH "Previously acquired fixed Q5_K_M model")
if(NOT BEANKEY_MODEL)
  set(BEANKEY_MODEL "${CMAKE_CURRENT_SOURCE_DIR}/crates/converter/assets/model/ggml-model-Q5_K_M.gguf")
endif()
set(dictionary "${CMAKE_CURRENT_SOURCE_DIR}/crates/converter/assets/dictionary")
foreach(asset IN ITEMS
    "${BEANKEY_MODEL}"
    "${dictionary}/azookeyKanaKanji/Dictionary/louds/charID.chid"
    "${dictionary}/azookeyKanaKanji/Dictionary/mm.binary"
    "${dictionary}/azookeyEmoji/EmojiDictionary/emoji_all_E17.0.txt"
    "${dictionary}/hunspellEnglish/en_US.aff" "${dictionary}/hunspellEnglish/en_US.dic"
    "${dictionary}/hunspellGreek/el_GR.aff" "${dictionary}/hunspellGreek/el_GR.dic")
  if(NOT EXISTS "${asset}")
    message(FATAL_ERROR "Missing fixed asset: ${asset}. Initialize submodules and provide -DBEANKEY_MODEL=<acquired model>.")
  endif()
endforeach()
set_property(DIRECTORY APPEND PROPERTY CMAKE_CONFIGURE_DEPENDS "${BEANKEY_MODEL}")
file(SHA256 "${BEANKEY_MODEL}" model_hash)
if(NOT model_hash STREQUAL "29c223d4c23327b80fd13ebb5ab2555057a46317997d5da391584ffbef0db673")
  message(FATAL_ERROR "BEANKEY_MODEL is not the fixed Q5_K_M GGUF (an unhydrated Git LFS pointer is not a model)")
endif()

set(BEANKEY_CARGO_TARGET_DIR "${CMAKE_CURRENT_BINARY_DIR}/cargo" CACHE PATH "Cargo build directory")
set(cargo_profile release)
set(cargo_flags --release)
if(CMAKE_BUILD_TYPE STREQUAL "Debug")
  set(cargo_profile debug)
  set(cargo_flags "")
endif()
set(daemon_binary "${BEANKEY_CARGO_TARGET_DIR}/${cargo_profile}/beankey-daemon")
add_custom_target(beankey-daemon ALL
  COMMAND "${CARGO_EXECUTABLE}" build --locked --package beankey-daemon
    --target-dir "${BEANKEY_CARGO_TARGET_DIR}" ${cargo_flags}
  BYPRODUCTS "${daemon_binary}"
  WORKING_DIRECTORY "${CMAKE_CURRENT_SOURCE_DIR}"
  USES_TERMINAL VERBATIM)

# Both the local package and Nix outputs use these same install rules.
function(install_beankey_runtime prefix component)
  install(PROGRAMS "${daemon_binary}" DESTINATION "${prefix}bin" COMPONENT "${component}")
  install(DIRECTORY "${dictionary}/azookeyKanaKanji/Dictionary/"
    DESTINATION "${prefix}share/beankey/dictionary" COMPONENT "${component}")
  install(FILES "${dictionary}/azookeyEmoji/EmojiDictionary/emoji_all_E17.0.txt"
    DESTINATION "${prefix}share/beankey" RENAME emoji-dictionary COMPONENT "${component}")
  install(FILES "${BEANKEY_MODEL}" DESTINATION "${prefix}share/beankey" RENAME model COMPONENT "${component}")
  install(DIRECTORY "${dictionary}/hunspellEnglish/" DESTINATION "${prefix}share/beankey/hunspell-english" COMPONENT "${component}")
  install(DIRECTORY "${dictionary}/hunspellGreek/" DESTINATION "${prefix}share/beankey/hunspell-greek" COMPONENT "${component}")
  install(FILES "${CMAKE_CURRENT_SOURCE_DIR}/crates/converter/assets/tokenizer/tokenizer.json"
    DESTINATION "${prefix}share/beankey" COMPONENT "${component}")
  install(FILES LICENSE THIRD_PARTY_NOTICES.txt DESTINATION "${prefix}share/licenses/beankey" COMPONENT "${component}")
endfunction()
install_beankey_runtime("" Daemon)

set(CMAKE_INSTALL_DEFAULT_COMPONENT_NAME Frontend)
if(APPLE)
  add_subdirectory(macos)
  set(bundle_path "Library/Input Methods/beanKey.app")
  install_beankey_runtime("${bundle_path}/Contents/Resources/beankey/" Frontend)
  set(BEANKEY_NIX_STORE "" CACHE FILEPATH "Optional nix-store executable for GC retention")
  configure_file(macos/install.sh.in beankey-install @ONLY)
  install(PROGRAMS "${CMAKE_CURRENT_BINARY_DIR}/beankey-install" DESTINATION bin COMPONENT Frontend)
  set(frontend_targets beanKey beankey-register-input-method)
else()
  set(BEANKEY_DAEMON_PATH "${CMAKE_INSTALL_PREFIX}/bin/beankey-daemon" CACHE FILEPATH "Installed daemon executable")
  add_subdirectory(fcitx5)
  set(frontend_targets fcitx5-beankey)
endif()

set(package "${CMAKE_CURRENT_BINARY_DIR}/package")
add_custom_target(beankey-package ALL
  COMMAND "${CMAKE_COMMAND}" -E rm -rf "${package}"
  COMMAND "${CMAKE_COMMAND}" --install "${CMAKE_CURRENT_BINARY_DIR}" --prefix "${package}"
  DEPENDS beankey-daemon ${frontend_targets}
  USES_TERMINAL VERBATIM)

if(BUILD_TESTING)
  set(test_runtime "${package}")
  if(APPLE)
    set(test_runtime "${package}/${bundle_path}/Contents/Resources/beankey")
  endif()
  add_test(NAME beankey-rust
    COMMAND "${CMAKE_COMMAND}" -E env
      "BEANKEY_TEST_MODEL=${test_runtime}/share/beankey/model"
      "BEANKEY_TEST_PACKAGE=${test_runtime}"
      "${CARGO_EXECUTABLE}" test --locked --workspace --all-targets
      --target-dir "${BEANKEY_CARGO_TARGET_DIR}" ${cargo_flags})
  set_tests_properties(beankey-rust PROPERTIES
    WORKING_DIRECTORY "${CMAKE_CURRENT_SOURCE_DIR}")
endif()
