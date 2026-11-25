#include "reveal.h"

#include <QDesktopServices>
#include <QFileInfo>
#include <QStringList>
#include <QUrl>

#ifdef Q_OS_MACOS
#include <QProcess>
#else
#include <QDBusMessage>
#include <QDBusPendingCall>
#include <QDBusPendingCallWatcher>
#include <QObject>
#endif

#include <utility>

namespace {

void openFolder(const QString &folder) { QDesktopServices::openUrl(QUrl::fromLocalFile(folder)); }

} // namespace

void revealFile(const QString &file, const Reveal &how) {
    how.showFile(file,
                 [open = how.openFolder, folder = QFileInfo(file).absolutePath()](bool shown) {
                     if (!shown) {
                         open(folder);
                     }
                 });
}

#ifdef Q_OS_MACOS

Reveal platformReveal() {
    return Reveal{[](const QString &file, std::function<void(bool shown)> done) {
                      done(QProcess::startDetached(QStringLiteral("/usr/bin/open"),
                                                   {QStringLiteral("-R"), file}));
                  },
                  openFolder};
}

#else

Reveal platformReveal() {
    return Reveal{[](const QString &file, std::function<void(bool shown)> done) {
                      showItems(file, std::move(done), QDBusConnection::sessionBus());
                  },
                  openFolder};
}

void showItems(const QString &file, std::function<void(bool shown)> done, QDBusConnection bus) {
    if (!bus.isConnected()) {
        done(false);
        return;
    }
    QDBusMessage call = QDBusMessage::createMethodCall(
        QStringLiteral("org.freedesktop.FileManager1"),
        QStringLiteral("/org/freedesktop/FileManager1"),
        QStringLiteral("org.freedesktop.FileManager1"), QStringLiteral("ShowItems"));
    call << QStringList{fileUri(file)} << QString();
    // At the 5 s timeout libdbus ends the call with an error, and drops any later reply: the
    // watcher finishes once either way, and deletes itself then.
    auto *answer = new QDBusPendingCallWatcher(bus.asyncCall(call, 5000));
    QObject::connect(answer, &QDBusPendingCallWatcher::finished, answer,
                     [done = std::move(done)](QDBusPendingCallWatcher *watcher) {
                         done(!watcher->isError());
                         watcher->deleteLater();
                     });
}

QString fileUri(const QString &file) {
    return QUrl::fromLocalFile(file).toString(QUrl::FullyEncoded);
}

#endif
