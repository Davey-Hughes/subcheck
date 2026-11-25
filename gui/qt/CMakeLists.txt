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
# warnings, which it does not pass; its header, which our code includes, passes them, all but
# -Wmaybe-uninitialized under GCC in the files the block after the window's tests names. The
# staticlib is named outright: Corrosion's plain subcheck_ffi is the cdylib whenever
# BUILD_SHARED_LIBS is on.
add_library(subcheck_bindings STATIC ${GENERATED})
target_include_directories(subcheck_bindings PUBLIC "${BINDINGS_DIR}")
target_link_libraries(subcheck_bindings PUBLIC subcheck_ffi-static Threads::Threads)
# A staticlib carries no link line of its own, and Corrosion adds only what Rust's std needs. On
# macOS the verifier that core checks a server's certificate with (rustls-platform-verifier) calls
# into these two frameworks, which this lists when run on a Mac:
# `cargo rustc -p subcheck-ffi --lib --crate-type staticlib -- --print native-static-libs`
if(APPLE)
    target_link_libraries(subcheck_ffi-static INTERFACE
        "-framework Security" "-framework CoreFoundation")
endif()

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
    QML_FILES qml/Main.qml qml/Sidebar.qml qml/FindingRow.qml qml/DetailsPanel.qml
            qml/PlayIndicator.qml qml/ChangeBlock.qml qml/SettingsWindow.qml qml/LoadingArc.qml
            qml/ListScrollBar.qml qml/CompactRow.qml qml/Disclosure.qml qml/KeyField.qml
            qml/FindBox.qml qml/WholeMenu.qml qml/VideoPane.qml qml/ControlBar.qml
            qml/PlayGlyph.qml
    SOURCES src/findings_model.h src/findings_model.cpp src/session_bridge.h
            src/session_bridge.cpp src/status_words.h src/status_words.cpp src/player_port.h
            src/player_port.cpp src/mpv_item.h src/mpv_item.cpp src/overloaded.h src/spliced.h
            src/words.h src/words.cpp src/sidebar_models.h src/sidebar_models.cpp
            src/detail_view.h src/detail_view.cpp src/window_settings.h src/window_settings.cpp
            src/menu_keys.h src/menu_keys.cpp src/loading_flag.h src/loading_flag.cpp
            src/ending.h src/ending.cpp src/reveal.h src/reveal.cpp src/press_away.h
            src/press_away.cpp src/key_names.h src/key_names.cpp)
target_include_directories(subcheck_window PUBLIC src)
target_link_libraries(subcheck_window PUBLIC subcheck_bindings Qt6::Quick MpvQt::MpvQt)
# Show File in Folder asks the file manager over the session bus on Linux (reveal.h); macOS has the
# Finder's `open -R`, and no D-Bus. Public, as reveal.h's showItems takes a bus.
if(NOT APPLE)
    find_package(Qt6 6.12 REQUIRED COMPONENTS DBus)
    target_link_libraries(subcheck_window PUBLIC Qt6::DBus)
endif()
subcheck_warnings(subcheck_window)

# The module's plugin, which registers its types: main.cpp imports it by name.
qt_add_executable(subcheck-qt src/main.cpp)
target_link_libraries(subcheck-qt PRIVATE subcheck_window subcheck_windowplugin)
subcheck_warnings(subcheck-qt)
install(TARGETS subcheck-qt)

enable_testing()

# Every test's limit, in seconds, but every Qt test binary subcheck_window_test makes: their seed
# steps keep this one, and the binaries have their own below. A session that never sends its next
# event fails its test, instead of holding CTest, and the CI jobs, for good.
set(SUBCHECK_TEST_TIMEOUT 60)
# The seeded window tests' limit: one process runs every function of a window test, so its limit
# covers them all, and is raised by hand as they grow; a session that hangs still ends the job.
set(SUBCHECK_WINDOW_TEST_TIMEOUT 300)

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
# decides findings needs a run of its own: CTest seeds a fixture once per run of the tests. A
# recheck reads the settings file, as the seed's scans do, so every test with a store of its own
# names one beside it too, which nothing writes (SUBCHECK_CONFIG): none reads the user's.
set(SEEDED "${CMAKE_BINARY_DIR}/seeded")
add_test(NAME seed_a_run
    COMMAND "${CMAKE_COMMAND}" -D "SUBCHECK=$<TARGET_FILE:subcheck>" -D "REPO_ROOT=${REPO_ROOT}"
            -D "DIR=${SEEDED}" -P "${CMAKE_CURRENT_SOURCE_DIR}/tests/seed.cmake")
set_tests_properties(seed_a_run PROPERTIES
    FIXTURES_SETUP seeded_run TIMEOUT ${SUBCHECK_TEST_TIMEOUT})
set(SEEDED_ENV "SUBCHECK_DB=${SEEDED}/subcheck.db" "XDG_STATE_HOME=${SEEDED}/state"
    "SUBCHECK_CONFIG=${SEEDED}/config.toml")

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
set(env "SUBCHECK_DB=${dir}/subcheck.db" "XDG_STATE_HOME=${dir}/state"
    "SUBCHECK_CONFIG=${dir}/config.toml")
set_tests_properties(bindings_smoke PROPERTIES
    ENVIRONMENT "${env}" TIMEOUT ${SUBCHECK_TEST_TIMEOUT})

# The list's model, on the splices it is given, with no session. Offscreen, as every Qt test here
# is, so that none opens a window on the desktop of whoever runs them.
qt_add_executable(findings_model_test tests/findings_model_test.cpp)
target_link_libraries(findings_model_test PRIVATE subcheck_window Qt6::Test)
subcheck_warnings(findings_model_test)
add_test(NAME findings_model_test COMMAND findings_model_test)
set_tests_properties(findings_model_test PROPERTIES
    ENVIRONMENT QT_QPA_PLATFORM=offscreen TIMEOUT ${SUBCHECK_TEST_TIMEOUT})

# The window's code that needs no session, each test on its own: the status bar's words for each
# message, the window's other words and colours, the sidebar's lists, the details panel's words,
# what the window remembers, when the busy animation over the video shows, how the window ends
# once review has, and how a file is shown in the file manager.
foreach(name IN ITEMS status_words_test words_test sidebar_models_test detail_view_test
                      window_settings_test loading_flag_test ending_test reveal_test)
    qt_add_executable(${name} tests/${name}.cpp)
    target_link_libraries(${name} PRIVATE subcheck_window Qt6::Test)
    subcheck_warnings(${name})
    add_test(NAME ${name} COMMAND ${name})
    set_tests_properties(${name} PROPERTIES
        ENVIRONMENT QT_QPA_PLATFORM=offscreen TIMEOUT ${SUBCHECK_TEST_TIMEOUT})
endforeach()

# Two screens for the tests of the window's size, a large one and a small one beside it: the
# offscreen platform's own is 800 by 800, smaller than the window's default.
set(TEST_SCREENS "${CMAKE_BINARY_DIR}/test-screens.json")
file(WRITE "${TEST_SCREENS}"
    "{\"screens\": [{\"name\": \"large\", \"x\": 0, \"y\": 0, \"width\": 1920, \"height\": 1080, "
    "\"logicalDpi\": 96, \"logicalBaseDpi\": 96, \"dpr\": 1}, {\"name\": \"small\", \"x\": 1920, "
    "\"y\": 0, \"width\": 1024, \"height\": 600, \"logicalDpi\": 96, \"logicalBaseDpi\": 96, "
    "\"dpr\": 1}]}\n")
set_tests_properties(window_settings_test PROPERTIES
    ENVIRONMENT "QT_QPA_PLATFORM=offscreen:configfile=${TEST_SCREENS}")
# reveal_test asks only a bus of its own, which nothing listens on. Should a test of it ever reach
# the session bus, libdbus, with no address given, would find a desktop's in the runtime dir: there
# is none.
set_property(TEST reveal_test APPEND PROPERTY ENVIRONMENT_MODIFICATION XDG_RUNTIME_DIR=unset:)

# The same two screens with the small one primary, for a window that comes back to a larger screen
# than the one it starts on. The offscreen platform orders screens by name and the first is the
# primary, so the names say which that is.
set(small_primary_screens "${CMAKE_BINARY_DIR}/test-screens-small-primary.json")
file(WRITE "${small_primary_screens}"
    "{\"screens\": [{\"name\": \"a-small\", \"x\": 0, \"y\": 0, \"width\": 1024, \"height\": 600, "
    "\"logicalDpi\": 96, \"logicalBaseDpi\": 96, \"dpr\": 1}, {\"name\": \"b-large\", \"x\": 1024, "
    "\"y\": 0, \"width\": 1920, \"height\": 1080, \"logicalDpi\": 96, \"logicalBaseDpi\": 96, "
    "\"dpr\": 1}]}\n")
qt_add_executable(window_settings_small_primary_test tests/window_settings_small_primary_test.cpp)
target_link_libraries(window_settings_small_primary_test PRIVATE subcheck_window Qt6::Test)
subcheck_warnings(window_settings_small_primary_test)
add_test(NAME window_settings_small_primary_test COMMAND window_settings_small_primary_test)
set_tests_properties(window_settings_small_primary_test PROPERTIES
    ENVIRONMENT "QT_QPA_PLATFORM=offscreen:configfile=${small_primary_screens}"
    TIMEOUT ${SUBCHECK_TEST_TIMEOUT})

# A test that makes a GUI application refuses to start anywhere Qt could draw on a desktop
# (tests/offscreen_main.h): on a platform that is not offscreen, and with a display to open.
add_test(NAME words_test_refuses_a_desktop
    COMMAND "${CMAKE_COMMAND}" -D "TEST=$<TARGET_FILE:words_test>"
            -P "${CMAKE_CURRENT_SOURCE_DIR}/tests/refuses.cmake")
set_tests_properties(words_test_refuses_a_desktop PROPERTIES
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
        "SUBCHECK_CONFIG=${dir}/config.toml" "SUBCHECK_TEST_MEDIA=${dir}/media"
        "SUBCHECK_TEST_CLEAN=${dir}/clean" ${TEST_MPV_ENV} QT_QPA_PLATFORM=offscreen)
    set_tests_properties(${name} PROPERTIES
        FIXTURES_REQUIRED ${name}_run ENVIRONMENT "${env}"
        TIMEOUT ${SUBCHECK_WINDOW_TEST_TIMEOUT})
endfunction()

subcheck_window_test(session_bridge_test)
subcheck_window_test(window_test)
# window_test checks the window's default size and its columns, which the large screen holds.
set_property(TEST window_test APPEND PROPERTY ENVIRONMENT_MODIFICATION
             "QT_QPA_PLATFORM=set:offscreen:configfile=${TEST_SCREENS}")

# With optimisation, GCC reports members of the bindings' nested variants as maybe uninitialized
# on the path a throwing copy takes, where they are never read (GCC PR 114592), and under -Werror
# that false report stops a Release build. These files copy those types: the window library's moc
# file, in QMetaType's copy of PlayerReport, and the tests, in their copies of PlayerReport and
# Event. The rest of our code keeps the check: a file that comes to copy them fails GCC's Release
# build the same way, and is added here. A multi-config generator names the moc file per
# configuration, which this does not reach. GCC alone, as Clang has no such warning.
if(CMAKE_CXX_COMPILER_ID STREQUAL "GNU")
    set_property(SOURCE "${CMAKE_CURRENT_BINARY_DIR}/subcheck_window_autogen/mocs_compilation.cpp"
                        tests/player_port_test.cpp tests/window_test.cpp
                 TARGET_DIRECTORY subcheck_window
                 APPEND PROPERTY COMPILE_OPTIONS -Wno-maybe-uninitialized)
endif()

# A file of ours with a read GCC can only call maybe uninitialized must fail to build, so that the
# check, off for the files above, cannot go from all of our code unseen. At -O2, as the warning
# runs only with optimisation: the Debug build checks it too. GCC alone, as Clang has no such
# warning. Colour off, as escape codes around the bracket would stop the test's pattern matching.
if(CMAKE_CXX_COMPILER_ID STREQUAL "GNU")
    add_library(maybe_uninitialized_canary OBJECT EXCLUDE_FROM_ALL
        tests/maybe_uninitialized_canary.cpp)
    set_target_properties(maybe_uninitialized_canary PROPERTIES AUTOMOC OFF AUTOUIC OFF)
    subcheck_warnings(maybe_uninitialized_canary)
    target_compile_options(maybe_uninitialized_canary PRIVATE -O2 -fdiagnostics-color=never)
    add_test(NAME maybe_uninitialized_is_checked
        COMMAND "${CMAKE_COMMAND}" --build "${CMAKE_BINARY_DIR}"
                --target maybe_uninitialized_canary)
    set_tests_properties(maybe_uninitialized_is_checked PROPERTIES
        PASS_REGULAR_EXPRESSION "\\[-Werror=maybe-uninitialized\\]"
        TIMEOUT ${SUBCHECK_TEST_TIMEOUT})
endif()

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
        "SUBCHECK_CONFIG=${dir}/config.toml" "SUBCHECK_TEST_MEDIA=${dir}/media"
        QT_QPA_PLATFORM=offscreen)
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
    COMMAND "${CMAKE_COMMAND}" -D "APP=$<TARGET_FILE:subcheck-qt>" -D ARGS=no-such-run
            -D "SAYS=subcheck-qt: no run named no-such-run"
            -P "${CMAKE_CURRENT_SOURCE_DIR}/tests/app_starts.cmake")
set_tests_properties(app_starts PROPERTIES
    FIXTURES_REQUIRED seeded_run
    ENVIRONMENT "${SEEDED_ENV};${TEST_MPV_ENV};QT_QPA_PLATFORM=offscreen"
    TIMEOUT ${SUBCHECK_TEST_TIMEOUT})
# Given no arguments, in a store with no runs: nothing to open, and the app says why.
set(dir "${CMAKE_BINARY_DIR}/nothing-to-open")
add_test(NAME app_starts_with_nothing_to_open
    COMMAND "${CMAKE_COMMAND}" -D "APP=$<TARGET_FILE:subcheck-qt>" -D "DIR=${dir}"
            -D "SAYS=subcheck-qt: no runs yet"
            -P "${CMAKE_CURRENT_SOURCE_DIR}/tests/app_starts.cmake")
set(env "SUBCHECK_DB=${dir}/subcheck.db" "XDG_STATE_HOME=${dir}/state"
    "SUBCHECK_CONFIG=${dir}/config.toml" ${TEST_MPV_ENV} QT_QPA_PLATFORM=offscreen)
set_tests_properties(app_starts_with_nothing_to_open PROPERTIES
    ENVIRONMENT "${env}" TIMEOUT ${SUBCHECK_TEST_TIMEOUT})

# The tests on real mpv, each on an X display of its own: the pane as the player's mpv
# (tests/pane_test.cpp), on timing.mkv and ties.mkv, two fixtures with video; and the app end to
# end (tests/end_to_end_test.cpp), over runs of timing.mkv and apply.mkv, fixtures with video, the
# second with a typo to edit. Linux only, as Xvfb and xdotool are X11's: the gui job runs them,
# and gui-macos does not.
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
        "SUBCHECK_CONFIG=${dir}/config.toml" "SUBCHECK_TEST_MEDIA=${dir}/media"
        "SUBCHECK_TEST_DIR=${dir}"
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
# And every test draws the window in Fusion, whatever style the desktop would pick, so that the
# window's sizes are the same on every machine, gui-macos's among them; a person's run sees the
# platform's own.
get_property(tests DIRECTORY PROPERTY TESTS)
foreach(test IN LISTS tests)
    set_property(TEST ${test} APPEND PROPERTY ENVIRONMENT_MODIFICATION ${unset_desktop_session}
                 QT_QUICK_CONTROLS_STYLE=set:Fusion)
endforeach()
