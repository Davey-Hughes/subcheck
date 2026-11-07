# subcheck's Qt app: the C++ bindings to ffi/ (the subcheck-ffi crate), generated at build time
# and never committed, and the tests that drive a real session through them.
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

# Our own code is held to these, and a warning in it fails the build.
function(subcheck_warnings target)
    target_compile_options(${target} PRIVATE -Wall -Wextra -Werror)
endfunction()

enable_testing()

# Every test's limit, in seconds: a session that never sends its next event fails its test,
# instead of holding CTest, and the CI jobs, for good.
set(SUBCHECK_TEST_TIMEOUT 60)

# The tests' run, made by the real `subcheck scan` with core's fake LLM over a copy of a fixture,
# in a store of the build directory's own: a session given neither variable finds the user's. The
# smoke decides a finding in it, so a test that counts on the run as seeded needs a copy of its
# own, or a RESOURCE_LOCK it shares with the smoke: under `ctest -j`, the order would decide.
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
