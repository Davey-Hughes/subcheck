// Review's message as the window's status line says it.
#pragma once

#include "subcheck_ffi.hpp"

#include <QString>

// `status` in words: the terminal's, until the window words its own.
QString statusWords(const subcheck_ffi::Status &status);

// A decision in a word, as the terminal's list says it (core's `list::label`): the status line's,
// and the list model's name for a row's colours, so that the window holds the words once.
QString stateWord(subcheck_ffi::State state);
