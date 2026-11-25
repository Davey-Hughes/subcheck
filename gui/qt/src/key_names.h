// The names the window gives keys where Qt's own would not do: Return is ⏎, the return symbol, so
// that the word "Return" beside a menu item's words does not read as more of them. Qt names a key
// for a person (QKeySequence's NativeText) through QCoreApplication::translate, in the context
// "QShortcut", asking the translators installed, the one installed last first. This one answers for
// Return alone, so that Qt's names stand for every other key, the keypad's Enter among them, and
// for the modifiers: Shift+Return is Shift+⏎. Made after any other translator, as another that
// named Return would otherwise be asked first, and before the window's QML, which reads each name
// once, as it loads. A key written in the window's code, as "Return", is read by Qt's untranslated
// names too, so it still names the key. macOS names Return by a symbol of its own, ↵, and asks no
// translator for it.
#pragma once

#include <QString>
#include <QTranslator>

class KeyNames : public QTranslator {
public:
    // Installed on the application, which must exist, until it is destroyed.
    KeyNames();

    QString translate(const char *context, const char *sourceText,
                      const char *disambiguation = nullptr, int n = -1) const override;
    // Not empty, so that installing it tells the application's objects that names changed, as a
    // translator with names to give does.
    bool isEmpty() const override;
};
