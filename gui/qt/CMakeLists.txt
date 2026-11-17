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
# The video pane: MpvQt, as Arch packages it and the macOS job builds it.
find_package(MpvQt 1.2.1 REQUIRED)

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
qt_add_qml_module(subcheck_window
    URI Subcheck
    VERSION 1.0
    OUTPUT_DIRECTORY "${CMAKE_BINARY_DIR}/qml/Subcheck"
    QML_FILES qml/Main.qml
    SOURCES src/findings_model.h src/findings_model.cpp src/session_bridge.h
            src/session_bridge.cpp src/status_words.h src/status_words.cpp src/player_port.h
            src/player_port.cpp src/mpv_item.h src/mpv_item.cpp src/overloaded.h)
target_include_directories(subcheck_window PUBLIC src)
target_link_libraries(subcheck_window PUBLIC subcheck_bindings Qt6::Quick MpvQt::MpvQt)
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

# The mpv config of every test that makes the video pane, in place of the user's: MpvQt reads
# mpvqt/mpvqt.conf in Qt's config location. On Linux that is XDG_CONFIG_HOME. On macOS it is
# ~/Library/Preferences, under the home CoreFoundation takes from CFFIXED_USER_HOME, which the tests
# there get as a home of the build directory's, with the same file where MpvQt looks under it.
# window_test stops when Qt's config location is not the test's, so that a redirect that does not
# take fails loudly. No sound, whatever a test plays.
set(TEST_CONFIG "${CMAKE_BINARY_DIR}/test-config")
file(WRITE "${TEST_CONFIG}/mpvqt/mpvqt.conf" "ao=null\n")
set(TEST_MPV_ENV "XDG_CONFIG_HOME=${TEST_CONFIG}")
if(APPLE)
    set(TEST_HOME "${CMAKE_BINARY_DIR}/test-home")
    file(WRITE "${TEST_HOME}/Library/Preferences/mpvqt/mpvqt.conf" "ao=null\n")
    list(APPEND TEST_MPV_ENV "CFFIXED_USER_HOME=${TEST_HOME}")
endif()

# A run made by the real `subcheck scan` with core's fake LLM over a copy of a fixture, in a store
# of the build directory's own: a session given neither variable finds the user's. A test that
# decides findings needs a run of its own: CTest seeds a fixture once per run of the tests.
set(SEEDED "${CMAKE_BINARY_DIR}/seeded")
add_test(NAME seed_a_run
    COMMAND "${CMAKE_COMMAND}" -D "SUBCHECK=$<TARGET_FILE:subcheck>" -D "REPO_ROOT=${REPO_ROOT}"
            -D "DIR=${SEEDED}" -P "${CMAKE_CURRENT_SOURCE_DIR}/tests/seed.cmake")
set_tests_properties(seed_a_run PROPERTIES
    FIXTURES_SETUP seeded_run TIMEOUT ${SUBCHECK_TEST_TIMEOUT})
set(SEEDED_ENV "SUBCHECK_DB=${SEEDED}/subcheck.db" "XDG_STATE_HOME=${SEEDED}/state")

# The smoke decides findings and checks a file again, so its run is seeded at each of its starts,
# in a directory of its own (tests/seeded.cmake): two copies of typos.mkv, so that a row has two
# members whose decisions can differ.
set(dir "${CMAKE_BINARY_DIR}/seeded-bindings_smoke")
add_executable(bindings_smoke tests/bindings_smoke.cpp)
target_link_libraries(bindings_smoke PRIVATE subcheck_bindings)
subcheck_warnings(bindings_smoke)
add_test(NAME bindings_smoke
    COMMAND "${CMAKE_COMMAND}" -D "SUBCHECK=$<TARGET_FILE:subcheck>" -D "REPO_ROOT=${REPO_ROOT}"
            -D "DIR=${dir}" -D COPIES=2 -D "TEST=$<TARGET_FILE:bindings_smoke>"
            -D "ARGS=${dir}/media" -P "${CMAKE_CURRENT_SOURCE_DIR}/tests/seeded.cmake")
set_tests_properties(bindings_smoke PROPERTIES
    ENVIRONMENT "SUBCHECK_DB=${dir}/subcheck.db;XDG_STATE_HOME=${dir}/state"
    TIMEOUT ${SUBCHECK_TEST_TIMEOUT})

# The list's model, on the splices it is given, with no session. Offscreen, as every Qt test here
# is, so that none opens a window on the desktop of whoever runs them.
qt_add_executable(findings_model_test tests/findings_model_test.cpp)
target_link_libraries(findings_model_test PRIVATE subcheck_window Qt6::Test)
subcheck_warnings(findings_model_test)
add_test(NAME findings_model_test COMMAND findings_model_test)
set_tests_properties(findings_model_test PROPERTIES
    ENVIRONMENT QT_QPA_PLATFORM=offscreen TIMEOUT ${SUBCHECK_TEST_TIMEOUT})

# The status line's words for each message, with no session.
qt_add_executable(status_words_test tests/status_words_test.cpp)
target_link_libraries(status_words_test PRIVATE subcheck_window Qt6::Test)
subcheck_warnings(status_words_test)
add_test(NAME status_words_test COMMAND status_words_test)
set_tests_properties(status_words_test PROPERTIES
    ENVIRONMENT QT_QPA_PLATFORM=offscreen TIMEOUT ${SUBCHECK_TEST_TIMEOUT})

# The player, on a fake mpv, with no session.
qt_add_executable(player_port_test tests/player_port_test.cpp)
target_link_libraries(player_port_test PRIVATE subcheck_window Qt6::Test)
subcheck_warnings(player_port_test)
add_test(NAME player_port_test COMMAND player_port_test)
set_tests_properties(player_port_test PROPERTIES
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
        ${TEST_MPV_ENV} QT_QPA_PLATFORM=offscreen)
    set_tests_properties(${name} PROPERTIES
        FIXTURES_REQUIRED ${name}_run ENVIRONMENT "${env}" TIMEOUT ${SUBCHECK_TEST_TIMEOUT})
endfunction()

subcheck_window_test(session_bridge_test)
subcheck_window_test(window_test)

# The bridge during a job, where review's end ends the process's groups, after which no session
# opens in the process: each test function in a process of its own, on a run seeded at its start
# (tests/seeded.cmake), whose file the test makes a FIFO. Built with the build directory, as the
# only place it takes that run from.
qt_add_executable(session_bridge_job_test tests/session_bridge_job_test.cpp)
target_compile_definitions(session_bridge_job_test PRIVATE
    "SUBCHECK_BUILD_DIR=\"${CMAKE_BINARY_DIR}\"")
target_link_libraries(session_bridge_job_test PRIVATE subcheck_window Qt6::Test)
subcheck_warnings(session_bridge_job_test)
foreach(function IN ITEMS quittingDuringADryRunStopsItAndSaysHowToFinishIt
                          aFailureDuringADryRunStillSaysHowToFinishIt
                          aGiveUpDuringADryRunStillSaysHowToFinishIt
                          aBridgeLetGoOfDuringADryRunStillSaysHowToFinishIt)
    set(dir "${CMAKE_BINARY_DIR}/seeded-${function}")
    add_test(NAME session_bridge_job_test.${function}
        COMMAND "${CMAKE_COMMAND}" -D "SUBCHECK=$<TARGET_FILE:subcheck>" -D "REPO_ROOT=${REPO_ROOT}"
                -D "DIR=${dir}" -D "TEST=$<TARGET_FILE:session_bridge_job_test>"
                -D "ARGS=${function}" -P "${CMAKE_CURRENT_SOURCE_DIR}/tests/seeded.cmake")
    set(env "SUBCHECK_DB=${dir}/subcheck.db" "XDG_STATE_HOME=${dir}/state"
        "SUBCHECK_TEST_MEDIA=${dir}/media" QT_QPA_PLATFORM=offscreen)
    set_tests_properties(session_bridge_job_test.${function} PROPERTIES
        ENVIRONMENT "${env}" TIMEOUT ${SUBCHECK_TEST_TIMEOUT})
endforeach()

# The app itself: one binary, and wired as main.cpp means it to be.
add_test(NAME one_binary
    COMMAND "${CMAKE_COMMAND}" -D "APP=$<TARGET_FILE:subcheck-qt>"
            -P "${CMAKE_CURRENT_SOURCE_DIR}/tests/one_binary.cmake")
set_tests_properties(one_binary PROPERTIES TIMEOUT ${SUBCHECK_TEST_TIMEOUT})
# main.cpp calls the library's finish(), which main must as it returns (main.cpp says why):
# subcheck-qt's objects need subcheck_ffi::finish() from the library. Deleting the call fails
# this; moving it elsewhere in main.cpp does not.
add_test(NAME app_calls_finish
    COMMAND "${CMAKE_COMMAND}" -D "OBJECTS=$<TARGET_OBJECTS:subcheck-qt>"
            -P "${CMAKE_CURRENT_SOURCE_DIR}/tests/calls_finish.cmake")
set_tests_properties(app_calls_finish PROPERTIES TIMEOUT ${SUBCHECK_TEST_TIMEOUT})
add_test(NAME app_starts
    COMMAND "${CMAKE_COMMAND}" -D "APP=$<TARGET_FILE:subcheck-qt>"
            -P "${CMAKE_CURRENT_SOURCE_DIR}/tests/app_starts.cmake")
set_tests_properties(app_starts PROPERTIES
    FIXTURES_REQUIRED seeded_run
    ENVIRONMENT "${SEEDED_ENV};${TEST_MPV_ENV};QT_QPA_PLATFORM=offscreen"
    TIMEOUT ${SUBCHECK_TEST_TIMEOUT})

# The tests on real mpv, each on an X display of its own: the pane as the player's mpv
# (tests/pane_test.cpp), on timing.mkv and ties.mkv, two fixtures with video; and the app end to
# end (tests/end_to_end_test.cpp), over a run of timing.mkv. Linux only, as Xvfb and xdotool are
# X11's: the gui job runs them, and gui-macos does not.
if(NOT APPLE)
    find_program(XVFB Xvfb REQUIRED)
    find_program(XDOTOOL xdotool REQUIRED)
    find_package(Qt6 6.12 REQUIRED COMPONENTS Network)
    qt_add_executable(pane_test tests/pane_test.cpp)
    target_link_libraries(pane_test PRIVATE subcheck_window subcheck_windowplugin Qt6::Test)
    subcheck_warnings(pane_test)
    add_test(NAME pane_test COMMAND pane_test)
    # A config of pane_test's own, as a user could write one, against which every load must still
    # land on its frame and play or stay paused as the player asks: tests/pane_test.cpp says how.
    set(PANE_CONFIG "${CMAKE_BINARY_DIR}/pane_test-config")
    file(WRITE "${PANE_CONFIG}/mpvqt/mpvqt.conf" "ao=null\nhr-seek=no\nreset-on-next-file=pause\n")
    set(env "SUBCHECK_TEST_VIDEO=${REPO_ROOT}/core/tests/fixtures/timing.mkv"
        "SUBCHECK_TEST_LINES=${REPO_ROOT}/core/tests/fixtures/ties.mkv"
        "SUBCHECK_TEST_DIR=${CMAKE_BINARY_DIR}/pane_test-run" "SUBCHECK_XVFB=${XVFB}"
        "XDG_CONFIG_HOME=${PANE_CONFIG}")
    set_tests_properties(pane_test PROPERTIES ENVIRONMENT "${env}" TIMEOUT ${SUBCHECK_TEST_TIMEOUT})

    # The test seeds its run itself, with seed.cmake, at each start: it decides a finding, so a run
    # of it after another, as `ctest --repeat` makes, needs the run as seeded again.
    set(dir "${CMAKE_BINARY_DIR}/seeded-end_to_end_test")
    qt_add_executable(end_to_end_test tests/end_to_end_test.cpp)
    # The seed deletes the directory it is given before it makes the run there: built in, this path
    # is the only one the test seeds, so an environment that names another folder fails the test
    # with nothing deleted.
    target_compile_definitions(end_to_end_test PRIVATE "SUBCHECK_E2E_DIR=\"${dir}\"")
    target_link_libraries(end_to_end_test PRIVATE Qt6::Network Qt6::Test)
    subcheck_warnings(end_to_end_test)
    add_test(NAME end_to_end_test COMMAND end_to_end_test)
    set(env "SUBCHECK_DB=${dir}/subcheck.db" "XDG_STATE_HOME=${dir}/state"
        "SUBCHECK_TEST_MEDIA=${dir}/media" "SUBCHECK_TEST_DIR=${dir}"
        "SUBCHECK_QT=$<TARGET_FILE:subcheck-qt>" "SUBCHECK_XVFB=${XVFB}"
        "SUBCHECK_XDOTOOL=${XDOTOOL}" "SUBCHECK_CMAKE=${CMAKE_COMMAND}"
        "SUBCHECK_SEED=${CMAKE_CURRENT_SOURCE_DIR}/tests/seed.cmake"
        "SUBCHECK_CLI=$<TARGET_FILE:subcheck>" "SUBCHECK_REPO=${REPO_ROOT}")
    set_tests_properties(end_to_end_test PROPERTIES
        ENVIRONMENT "${env}" TIMEOUT ${SUBCHECK_TEST_TIMEOUT})
endif()

# Every test runs with none of a desktop session's variables, as in CI, which has none. Qt's
# offscreen platform still opens $DISPLAY for OpenGL when it is set, so a run from a desktop's
# terminal would make GL contexts on the desktop's X server, and window_test's mpv, which offscreen
# never becomes ready, could become ready there; the bus, theme and session names would reach the
# desktop's too. pane_test and end_to_end_test name their own display after, for their own
# processes. The names are tests/desktop_session.h's exact ones, its twin, and the two change
# together; CMake cannot match the header's prefixes, KDE_ and XDG_SESSION_, so of those only the
# names Qt reads are here. Appended, so that a test's own modifications stay. The last statement in
# this file, as it reaches only the tests above it.
set(DESKTOP_SESSION_NAMES DISPLAY WAYLAND_DISPLAY PULSE_SERVER PIPEWIRE_REMOTE
    DBUS_SESSION_BUS_ADDRESS AT_SPI_BUS_ADDRESS XDG_CURRENT_DESKTOP QT_QPA_PLATFORMTHEME
    SESSION_MANAGER DESKTOP_SESSION GNOME_DESKTOP_SESSION_ID ICEAUTHORITY XAUTHORITY XDG_SEAT
    XDG_VTNR KDE_FULL_SESSION KDE_SESSION_VERSION XDG_SESSION_TYPE)
list(TRANSFORM DESKTOP_SESSION_NAMES APPEND "=unset:" OUTPUT_VARIABLE unset_desktop_session)
get_property(tests DIRECTORY PROPERTY TESTS)
foreach(test IN LISTS tests)
    set_property(TEST ${test} APPEND PROPERTY ENVIRONMENT_MODIFICATION ${unset_desktop_session})
endforeach()
