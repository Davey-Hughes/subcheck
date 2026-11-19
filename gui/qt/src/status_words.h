// Review's message as the window's status bar says it.
#pragma once

#include "subcheck_ffi.hpp"

#include <QString>

// `status` in the window's words: sentences that name review's actions as its menus do, never a
// key, with files by the names the list gives them.
QString statusWords(const subcheck_ffi::Status &status);
