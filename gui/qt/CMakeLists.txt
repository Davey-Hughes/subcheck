# subcheck's Qt app: the C++ bindings to ffi/ (the subcheck-ffi crate), generated at build time
# and never committed; the window over them, `subcheck-qt`; and the tests that drive real sessions
# through both.
cmake_minimum_required(VERSION 3.22)
project(subcheck-qt LANGUAGES CXX)

set(CMAKE_CXX_STANDARD 20)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CMAKE_CXX_EXTENSIONS OFF)

cmake_path(SET REPO_ROOT NORMALIZE "${CMAKE_CURRENT_SOURCE_DIR}/../..")

# Corrosion builds with the Rust that rustup picks for CMake's working directory, which can be
# outside the repository, out of rust-toolchain.toml's reach, and it keeps that Rust until the
# next configure. Asked from this directory, rustup picks the repository's, and a change to the
# file reconfigures.
set_property(DIRECTORY APPEND PROPERTY CMAKE_CONFIGURE_DEPENDS "${REPO_ROOT}/rust-toolchain.toml")
execute_process(COMMAND rustup show active-toolchain
    WORKING_DIRECTORY "${CMAKE_CURRENT_SOURCE_DIR}"
    OUTPUT_VARIABLE active_toolchain OUTPUT_STRIP_TRAILING_WHITESPACE COMMAND_ERROR_IS_FATAL ANY)
string(REGEX MATCH "^[^ ]+" Rust_TOOLCHAIN "${active_toolchain}")

find_package(Corrosion REQUIRED)
find_package(Threads REQUIRED)
# The bindgen, looked for in the root `cargo install` uses when cargo's config names no
# `install.root` (CARGO_INSTALL_ROOT, else CARGO_HOME, else ~/.cargo), whose copy's tag
# dev/check-gui.sh checks, then on PATH, which need not hold it. CMAKE_PREFIX_PATH and
# CMAKE_PROGRAM_PATH, when set, are searched first. Each configure looks again (NO_CACHE) unless
# UNIFFI_BINDGEN_CPP is given with -D, and the bindings below depend on the file it finds.
if(DEFINED ENV{CARGO_INSTALL_ROOT})
    set(cargo_root "$ENV{CARGO_INSTALL_ROOT}")
elseif(DEFINED ENV{CARGO_HOME})
    set(cargo_root "$ENV{CARGO_HOME}")
else()
    set(cargo_root "$ENV{HOME}/.cargo")
endif()
find_program(UNIFFI_BINDGEN_CPP uniffi-bindgen-cpp HINTS "${cargo_root}/bin" REQUIRED NO_CACHE)
# The Qt both CI jobs build with, Arch's and Homebrew's. The standard setup runs moc where it is
# needed, and gives a QML module's files the resource prefix that `loadFromModule` looks under.
find_package(Qt6 6.12 REQUIRED COMPONENTS Quick Test)
qt_standard_project_setup(REQUIRES 6.12)

# The gui CI job builds with this on, so that its one run of the tests also gives gcovr what they
# reached. Every target is instrumented; gcovr counts gui/qt/src/ alone.
option(SUBCHECK_COVERAGE "Build for gcov, as the gui CI job does" OFF)
if(SUBCHECK_COVERAGE)
    add_compile_options(--coverage)
    add_link_options(--coverage)
endif()

# `subcheck` is for the tests, which seed their runs with a real scan. LOCKED: a build never
# changes Cargo.lock.
corrosion_import_crate(MANIFEST_PATH "${REPO_ROOT}/Cargo.toml" CRATES subcheck-ffi subcheck LOCKED)

# The bindings, from the cdylib: library mode reads the interface out of it, and runs `cargo
# metadata`, which needs the repository as its working directory. A new cdylib, or a newer
# bindgen (a reinstall), regenerates them, and everything that includes them recompiles.
set(BINDINGS_DIR "${CMAKE_BINARY_DIR}/bindings")
set(GENERATED
    "${BINDINGS_DIR}/subcheck_ffi.cpp"
    "${BINDINGS_DIR}/subcheck_ffi.hpp"
    "${BINDINGS_DIR}/subcheck_ffi_scaffolding.hpp")
add_custom_command(
    OUTPUT ${GENERATED}
    COMMAND "${UNIFFI_BINDGEN_CPP}" --library "$<TARGET_FILE:subcheck_ffi-shared>"
            --out-dir "${BINDINGS_DIR}"
    DEPENDS subcheck_ffi-shared "$<TARGET_FILE:subcheck_ffi-shared>" "${UNIFFI_BINDGEN_CPP}"
    WORKING_DIRECTORY "${REPO_ROOT}"
    COMMENT "Generating the C++ bindings"
    VERBATIM)

# The generated .cpp, with the staticlib, so that the app is one binary. It is built without our
# warnings, which it does not pass; its header, which our code includes, does pass them. The
# staticlib is named outright: Corrosion's plain subcheck_ffi is the cdylib whenever
# BUILD_SHARED_LIBS is on.
add_library(subcheck_bindings STATIC ${GENERATED})
target_include_directories(subcheck_bindings PUBLIC "${BINDINGS_DIR}")
target_link_libraries(subcheck_bindings PUBLIC subcheck_ffi-static Threads::Threads)

# Our own code is held to these, and a warning in it fails the build. So is the C++ Qt generates
# for a target given them: moc's, and the QML module's type registrations, resources and compiled
# QML. The module's plugin, a target qt_add_qml_module makes, is not given them.
function(subcheck_warnings target)
    target_compile_options(${target} PRIVATE -Wall -Wextra -Werror)
endfunction()

# The window's C++ and QML, as the QML module Subcheck on a static library, which the app and the
# tests link alike. The QML reaches the C++ through the types the module registers, which qmllint
# checks it against, rather than through context properties, which it cannot see into. The
# module's files go to qml/Subcheck, whose parent qmllint is given as an import path, and not to
# Subcheck beside the build's other products: where a filesystem ignores case, as macOS's does by
# default, that is one name with the `subcheck` that Corrosion copies there and the tests' seed
# runs, and the copy then lands inside the module's directory.
qt_add_library(subcheck_window STATIC)
# Main.qml at the module's root, where `loadFromModule` looks, though it sits in qml/ here.
set_source_files_properties(qml/Main.qml PROPERTIES QT_RESOURCE_ALIAS Main.qml)
qt_add_qml_module(subcheck_window
    URI Subcheck
    VERSION 1.0
    OUTPUT_DIRECTORY "${CMAKE_BINARY_DIR}/qml/Subcheck"
    QML_FILES qml/Main.qml
    SOURCES src/findings_model.h src/findings_model.cpp src/session_bridge.h
            src/session_bridge.cpp)
target_include_directories(subcheck_window PUBLIC src)
target_link_libraries(subcheck_window PUBLIC subcheck_bindings Qt6::Quick)
subcheck_warnings(subcheck_window)

# The module's plugin, which registers its types: main.cpp imports it by name.
qt_add_executable(subcheck-qt src/main.cpp)
target_link_libraries(subcheck-qt PRIVATE subcheck_window subcheck_windowplugin)
subcheck_warnings(subcheck-qt)
install(TARGETS subcheck-qt)

enable_testing()

# Every test's limit, in seconds: a session that never sends its next event fails its test,
# instead of holding CTest, and the CI jobs, for good.
set(SUBCHECK_TEST_TIMEOUT 60)

# The smoke's run, made by the real `subcheck scan` with core's fake LLM over a copy of a fixture,
# in a store of the build directory's own: a session given neither variable finds the user's. The
# smoke decides a finding in it, so a test that counts on the run as seeded needs a run of its own:
# CTest seeds a fixture once per run of the tests, and a RESOURCE_LOCK would keep two tests from
# sharing the store at once, not the smoke's decision from being there when the other starts.
set(SEEDED "${CMAKE_BINARY_DIR}/seeded")
add_test(NAME seed_a_run
    COMMAND "${CMAKE_COMMAND}" -D "SUBCHECK=$<TARGET_FILE:subcheck>" -D "REPO_ROOT=${REPO_ROOT}"
            -D "DIR=${SEEDED}" -P "${CMAKE_CURRENT_SOURCE_DIR}/tests/seed.cmake")
set_tests_properties(seed_a_run PROPERTIES
    FIXTURES_SETUP seeded_run TIMEOUT ${SUBCHECK_TEST_TIMEOUT})
set(SEEDED_ENV "SUBCHECK_DB=${SEEDED}/subcheck.db" "XDG_STATE_HOME=${SEEDED}/state")

add_executable(bindings_smoke tests/bindings_smoke.cpp)
target_link_libraries(bindings_smoke PRIVATE subcheck_bindings)
subcheck_warnings(bindings_smoke)
add_test(NAME bindings_smoke COMMAND bindings_smoke "${SEEDED}/media")
set_tests_properties(bindings_smoke PROPERTIES
    FIXTURES_REQUIRED seeded_run ENVIRONMENT "${SEEDED_ENV}" TIMEOUT ${SUBCHECK_TEST_TIMEOUT})

# The list's model, on the splices it is given, with no session. Offscreen, as every Qt test here
# is, so that none opens a window on the desktop of whoever runs them.
qt_add_executable(findings_model_test tests/findings_model_test.cpp)
target_link_libraries(findings_model_test PRIVATE subcheck_window Qt6::Test)
subcheck_warnings(findings_model_test)
add_test(NAME findings_model_test COMMAND findings_model_test)
set_tests_properties(findings_model_test PROPERTIES
    ENVIRONMENT QT_QPA_PLATFORM=offscreen TIMEOUT ${SUBCHECK_TEST_TIMEOUT})

# A Qt test of the window's code, `tests/<name>.cpp`, on runs of its own (see the smoke's): two
# copies of typos.mkv, so that each of its two rows has two members to expand into; and a run with
# no findings. Their folders, the test's selectors, are SUBCHECK_TEST_MEDIA and SUBCHECK_TEST_CLEAN.
function(subcheck_window_test name)
    set(dir "${CMAKE_BINARY_DIR}/seeded-${name}")
    add_test(NAME seed_${name}
        COMMAND "${CMAKE_COMMAND}" -D "SUBCHECK=$<TARGET_FILE:subcheck>" -D "REPO_ROOT=${REPO_ROOT}"
                -D "DIR=${dir}" -D COPIES=2 -D CLEAN=1
                -P "${CMAKE_CURRENT_SOURCE_DIR}/tests/seed.cmake")
    set_tests_properties(seed_${name} PROPERTIES
        FIXTURES_SETUP ${name}_run TIMEOUT ${SUBCHECK_TEST_TIMEOUT})
    qt_add_executable(${name} tests/${name}.cpp)
    target_link_libraries(${name} PRIVATE subcheck_window subcheck_windowplugin Qt6::Test)
    subcheck_warnings(${name})
    add_test(NAME ${name} COMMAND ${name})
    set(env "SUBCHECK_DB=${dir}/subcheck.db" "XDG_STATE_HOME=${dir}/state"
        "SUBCHECK_TEST_MEDIA=${dir}/media" "SUBCHECK_TEST_CLEAN=${dir}/clean"
        QT_QPA_PLATFORM=offscreen)
    set_tests_properties(${name} PROPERTIES
        FIXTURES_REQUIRED ${name}_run ENVIRONMENT "${env}" TIMEOUT ${SUBCHECK_TEST_TIMEOUT})
endfunction()

subcheck_window_test(session_bridge_test)
subcheck_window_test(window_test)

# The app itself: one binary, and wired as main.cpp means it to be.
add_test(NAME one_binary
    COMMAND "${CMAKE_COMMAND}" -D "APP=$<TARGET_FILE:subcheck-qt>"
            -P "${CMAKE_CURRENT_SOURCE_DIR}/tests/one_binary.cmake")
set_tests_properties(one_binary PROPERTIES TIMEOUT ${SUBCHECK_TEST_TIMEOUT})
add_test(NAME app_starts
    COMMAND "${CMAKE_COMMAND}" -D "APP=$<TARGET_FILE:subcheck-qt>"
            -P "${CMAKE_CURRENT_SOURCE_DIR}/tests/app_starts.cmake")
set_tests_properties(app_starts PROPERTIES
    FIXTURES_REQUIRED seeded_run ENVIRONMENT "${SEEDED_ENV};QT_QPA_PLATFORM=offscreen"
    TIMEOUT ${SUBCHECK_TEST_TIMEOUT})
