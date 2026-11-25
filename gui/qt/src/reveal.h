// Show File in Folder: a file shown in the platform's file manager, its folder open with the file
// selected; failing that, its folder opened.
#pragma once

#include <QString>
#include <QtGlobal>

#ifndef Q_OS_MACOS
#include <QDBusConnection>
#endif

#include <functional>

// How a file is shown: `showFile` asks the file manager to show it selected, and calls `done` once
// with whether it did; `openFolder` opens a folder, as the desktop opens one.
struct Reveal {
    std::function<void(const QString &file, std::function<void(bool shown)> done)> showFile;
    std::function<void(const QString &folder)> openFolder;
};

// `file` shown by `how`, and once it says the file was not shown, the file's folder opened: a
// desktop may have no file manager that answers, or one that does not implement ShowItems, and
// the folder still takes the person most of the way. `how` can go before the answer comes. The
// folder is worked out from the file's path alone, reading no disk: a share gone slow would hold
// the window.
void revealFile(const QString &file, const Reveal &how);

// The platform's: on Linux, the file manager asked over the session bus (showItems); on macOS, the
// Finder, by `open -R`, whose start says whether it was asked. A folder by the desktop's handler
// of its URL.
Reveal platformReveal();

#ifndef Q_OS_MACOS
// The freedesktop file manager's ShowItems for `file`, asked on `bus`: `done` once, true on its
// reply, false on an error, after 5 s with none, or at once on a bus that is not connected. A
// reply after the 5 s is dropped. The call never blocks: a file manager that the bus starts for it
// can take seconds to answer, and the window would stop for as long. The bus is given, so that a
// test asks one of its own and never a desktop's: with no bus address set, libdbus still finds the
// desktop's in the runtime dir.
void showItems(const QString &file, std::function<void(bool shown)> done, QDBusConnection bus);

// `file` as the URI ShowItems takes: every character a URI may not hold as it is percent-encoded,
// a space and a non-ASCII letter among them, so that a strict file manager does not refuse it.
QString fileUri(const QString &file);
#endif
