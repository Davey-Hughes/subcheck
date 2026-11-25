#include "key_names.h"

#include <QByteArray>
#include <QCoreApplication>

KeyNames::KeyNames() { QCoreApplication::installTranslator(this); }

QString KeyNames::translate(const char *context, const char *sourceText, const char *disambiguation,
                            int) const {
    if (qstrcmp(context, "QShortcut") == 0 && qstrcmp(sourceText, "Return") == 0 &&
        qstrlen(disambiguation) == 0) {
        return QStringLiteral("⏎");
    }
    return {};
}

bool KeyNames::isEmpty() const { return false; }
