# The window as a macOS app, included from gui/qt/CMakeLists.txt on APPLE after subcheck-qt is
# made. In the build tree it is subcheck-qt.app, which the tests run through
# $<TARGET_FILE:subcheck-qt> as they run the bare executable elsewhere; cmake --install makes it
# Subcheck.app, with Qt, MpvQt, libmpv and the CLI inside and signed ad hoc, which runs on a Mac
# without Homebrew. The build tree's bundle keeps the executable's name, as a Subcheck.app there
# would be one name with Corrosion's subcheck beside it, macOS's disk ignoring case.

# The app's version is the workspace's, the first version line in Cargo.toml, which is
# [workspace.package]'s, so that the app and the CLI inside it never say different ones. A change
# to the file reconfigures.
set_property(DIRECTORY APPEND PROPERTY CMAKE_CONFIGURE_DEPENDS "${REPO_ROOT}/Cargo.toml")
file(STRINGS "${REPO_ROOT}/Cargo.toml" version_lines REGEX "^version = \"")
list(GET version_lines 0 version_line)
string(REGEX REPLACE "^version = \"([^\"]*)\".*$" "\\1" SUBCHECK_VERSION "${version_line}")
if(NOT SUBCHECK_VERSION MATCHES "^[0-9]+(\\.[0-9]+)*$")
    message(FATAL_ERROR "Cargo.toml's first version line, ${version_line}, gives no version")
endif()

# Info.plist.in names the app as macOS shows it and knows it; the executable keeps its own name,
# subcheck-qt, as Contents/MacOS/Subcheck would be one name with the CLI's subcheck there.
set_target_properties(subcheck-qt PROPERTIES
    MACOSX_BUNDLE TRUE
    MACOSX_BUNDLE_INFO_PLIST "${CMAKE_CURRENT_LIST_DIR}/Info.plist.in"
    MACOSX_BUNDLE_BUNDLE_NAME Subcheck
    MACOSX_BUNDLE_GUI_IDENTIFIER com.daveyhughes.subcheck
    MACOSX_BUNDLE_BUNDLE_VERSION "${SUBCHECK_VERSION}"
    MACOSX_BUNDLE_SHORT_VERSION_STRING "${SUBCHECK_VERSION}")

# Where finish-bundle.sh looks for what the deploy left out: MpvQt's prefix, three levels above the
# directory its package config is in, and Homebrew's, asked here, whose Cellar and brew licenses.sh
# reads too. A Mac without brew still configures, builds and runs the tests; its install fails,
# saying why.
cmake_path(GET MpvQt_DIR PARENT_PATH mpvqt_prefix)
cmake_path(GET mpvqt_prefix PARENT_PATH mpvqt_prefix)
cmake_path(GET mpvqt_prefix PARENT_PATH mpvqt_prefix)
find_program(SUBCHECK_BREW brew NO_CACHE)
set(brew_prefix "")
if(SUBCHECK_BREW)
    execute_process(COMMAND "${SUBCHECK_BREW}" --prefix
        OUTPUT_VARIABLE brew_prefix OUTPUT_STRIP_TRAILING_WHITESPACE COMMAND_ERROR_IS_FATAL ANY)
endif()

# The install, in order. A bundle an earlier install left half made goes first, so that nothing it
# held, such as a library the window no longer loads, is signed into this one. Then the build's
# bundle, and Qt's deploy for a QML app over it, which runs macdeployqt: the Qt frameworks and
# plugins, the QML modules qmlimportscanner finds the window's QML importing, and the libraries
# they and the window load. Then finish-bundle.sh: what the deploy left pointing outside the bundle,
# the CLI, the minimum macOS, the name and the signature. Each install path is DESTDIR's and the
# prefix's, as Qt's deploy takes it.
install(CODE "file(REMOVE_RECURSE \"\$ENV{DESTDIR}\${CMAKE_INSTALL_PREFIX}/subcheck-qt.app\")")
install(TARGETS subcheck-qt BUNDLE DESTINATION .)
qt_generate_deploy_qml_app_script(TARGET subcheck-qt OUTPUT_SCRIPT deploy_script NO_TRANSLATIONS)
install(SCRIPT "${deploy_script}")
install(CODE "
    execute_process(
        COMMAND \"${CMAKE_CURRENT_LIST_DIR}/finish-bundle.sh\"
                \"\$ENV{DESTDIR}\${CMAKE_INSTALL_PREFIX}/subcheck-qt.app\"
                \"$<TARGET_FILE:subcheck>\" \"${mpvqt_prefix}\" \"${brew_prefix}\" \"${REPO_ROOT}\"
        COMMAND_ERROR_IS_FATAL ANY)
")
