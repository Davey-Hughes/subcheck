// The bridge during a job, on real sessions: a dry run of the run seeded for the test, stalled on
// its file, made a FIFO no one writes to, as on a stalled mount. Review's end during a job ends the
// process's groups, after which no session opens in it, so CMake runs each test in a process of
// its own, on a run seeded afresh at its start (tests/seeded.cmake): typos.mkv, two rows of one
// finding each.
#include "offscreen_main.h"
#include "session_bridge.h"

#include <QFile>
#include <QRegularExpression>
#include <QSignalSpy>
#include <QTest>

#include <chrono>
#include <memory>
#include <optional>
#include <sstream>
#include <string>
#include <sys/stat.h>
#include <vector>

using namespace subcheck_ffi;

namespace {

// The seeded run's folder: the session's selector.
std::string media() { return qEnvironmentVariable("SUBCHECK_TEST_MEDIA").toStdString(); }

// A bridge on the seeded run, its end's text in `out` and `err`, and what its session last said
// of the job and the pane, and the first question it asked; the same short wait after a quit as
// the bridge's other tests. On the heap, so that a test can let it go before the rest.
struct Opened {
    std::ostringstream out;
    std::ostringstream err;
    PlayerPort player{std::chrono::milliseconds(100)};
    std::unique_ptr<SessionBridge> bridge = std::make_unique<SessionBridge>(
        std::vector<std::string>{media()}, player, out, err, std::chrono::seconds(1));
    std::optional<Job> job;
    std::optional<Pane> pane;
    std::optional<Question> asked;

    Opened() {
        QObject::connect(
            bridge.get(), &SessionBridge::eventArrived, bridge.get(), [this](const Event &e) {
                if (const auto *change = std::get_if<Event::kChanged>(&e.get_variant())) {
                    job = change->summary.job;
                    pane = change->summary.pane;
                    if (!asked) {
                        asked = change->summary.question;
                    }
                }
            });
    }

    // The first finding accepted, its file made a FIFO, and a dry run started, on that file. It
    // returns once a step after the dry run's on its file has been said: review records a step's
    // outro, which a close says, only after the step's change is on the stream, and takes the next
    // input only once it has, so the outro then says where the dry run is. Null then; else which
    // of its steps failed, for the test's failure to name.
    const char *stalled() {
        bridge->start();
        if (!QTest::qWaitFor([&] { return bridge->findings()->rowCount() == 2; })) {
            return "the run's two rows never came";
        }
        bridge->send(Action(Action::kAccept{}));
        FindingsModel *list = bridge->findings();
        if (!QTest::qWaitFor([&] {
                return list->data(list->index(0), FindingsModel::StateWordRole) == "Accepted";
            })) {
            return "the first finding was never accepted";
        }
        const QString file = QString::fromStdString(media()) + "/typos.mkv";
        if (!QFile::remove(file) || mkfifo(QFile::encodeName(file).constData(), 0600) != 0) {
            return "typos.mkv was not made a FIFO";
        }
        bridge->send(Action(Action::kApply{true}));
        if (!QTest::qWaitFor([&] {
                const auto *applying =
                    job ? std::get_if<Job::kApplying>(&job->get_variant()) : nullptr;
                const auto *on =
                    applying ? std::get_if<ApplyAt::kOn>(&applying->at.get_variant()) : nullptr;
                return on && on->file == "typos" && on->stage == ApplyStage::kPlan;
            })) {
            return "the dry run never reached typos.mkv's plan";
        }
        bridge->send(Action(Action::kShowPane{Pane::kFiles}));
        if (!QTest::qWaitFor([&] { return pane == Pane::kFiles; })) {
            return "the files pane never came";
        }
        return nullptr;
    }
};

// That `err` ends on how to finish the dry run.
void saysHowToFinish(const std::string &err) {
    QVERIFY2(err.find("dry run stopped on typos (plan), 0/1 files done:\n") != std::string::npos,
             err.c_str());
    QVERIFY2(err.ends_with(" --dry-run\n") &&
                 err.find("\nFinish it with: subcheck apply ") != std::string::npos,
             err.c_str());
}

} // namespace

class SessionBridgeJobTest : public QObject {
    Q_OBJECT

private slots:
    void initTestCase() {
        // A test decides a finding and makes a file of the run a FIFO, so its store and folder
        // may only be those CMake made for it in the build directory the test is built with: any
        // other, a folder of the user's say, fails the test here with nothing touched.
        const QString media = qEnvironmentVariable("SUBCHECK_TEST_MEDIA");
        const QRegularExpression ours("^" + QRegularExpression::escape(SUBCHECK_BUILD_DIR) +
                                      "/seeded-[A-Za-z]+/media$");
        QVERIFY2(ours.match(media).hasMatch() &&
                     qEnvironmentVariable("SUBCHECK_DB") == media.chopped(5) + "subcheck.db",
                 "run through ctest, on a run CMake seeds in " SUBCHECK_BUILD_DIR);
    }

    // The window's quit stops the job, as two `Interrupt`s do, asking first; and review's own
    // end says how to finish it: the window does not wait out the give-up.
    void quittingDuringADryRunStopsItAndSaysHowToFinishIt() {
        Opened o;
        QSignalSpy finished(o.bridge.get(), &SessionBridge::finished);
        const char *stuck = o.stalled();
        QVERIFY2(!stuck, stuck);
        o.bridge->quit();
        QTRY_COMPARE(finished.count(), 1);
        QCOMPARE(finished[0][0].toInt(), 0);
        saysHowToFinish(o.err.str());
        QCOMPARE(o.out.str(), "media: 1 accepted, 0 rejected, 1 pending\n");
        // What the first `Interrupt` asked, which the second answered.
        const auto *quit =
            o.asked ? std::get_if<Question::kQuit>(&o.asked->get_variant()) : nullptr;
        const auto *apply =
            quit ? std::get_if<Stopping::kApply>(&quit->stopping.get_variant()) : nullptr;
        const auto *on = apply ? std::get_if<ApplyAt::kOn>(&apply->at.get_variant()) : nullptr;
        QVERIFY(apply && apply->dry_run && apply->done == 0 && apply->queued == 1);
        QVERIFY(on && on->file == "typos" && on->stage == ApplyStage::kPlan);
    }

    // A failure lets review go: the close ends the job, and its end, which the bridge no longer
    // applies, still says how to finish the job, before the failure's words.
    void aFailureDuringADryRunStillSaysHowToFinishIt() {
        Opened o;
        QSignalSpy fatal(o.bridge.get(), &SessionBridge::fatalChanged);
        const char *stuck = o.stalled();
        QVERIFY2(!stuck, stuck);
        emit o.bridge->eventArrived(
            Event(Event::kChanged{Splice{3, 1, {}}, Summary{}, {}, {}, {}, {}}));
        QTRY_COMPARE(fatal.count(), 1);
        const std::string err = o.err.str();
        const std::string why = "subcheck-qt: the list is out of step with review\n";
        QVERIFY2(err.ends_with(why), err.c_str());
        saysHowToFinish(err.substr(0, err.size() - why.size()));
        QCOMPARE(o.out.str(), "");
    }

    // The session's events kept from the bridge, as from a core thread that cannot answer: the
    // quit gives up after its wait, and the end it lets go of still says how to finish the job,
    // before the give-up's words.
    void aGiveUpDuringADryRunStillSaysHowToFinishIt() {
        Opened o;
        QSignalSpy finished(o.bridge.get(), &SessionBridge::finished);
        const char *stuck = o.stalled();
        QVERIFY2(!stuck, stuck);
        QVERIFY(QObject::disconnect(o.bridge.get(), &SessionBridge::eventArrived, o.bridge.get(),
                                    nullptr));
        o.bridge->quit();
        QTRY_COMPARE_WITH_TIMEOUT(finished.count(), 1, 3000);
        QCOMPARE(finished[0][0].toInt(), 1);
        const std::string err = o.err.str();
        const std::string why =
            "subcheck-qt: review had not ended 1 s after quitting, so it was closed\n";
        QVERIFY2(err.ends_with(why), err.c_str());
        saysHowToFinish(err.substr(0, err.size() - why.size()));
        QCOMPARE(o.out.str(), "");
    }

    // A bridge let go of during the job, as by a window torn down without its quit: its close
    // ends the job, and its end still says how to finish it.
    void aBridgeLetGoOfDuringADryRunStillSaysHowToFinishIt() {
        Opened o;
        const char *stuck = o.stalled();
        QVERIFY2(!stuck, stuck);
        o.bridge.reset();
        saysHowToFinish(o.err.str());
        QCOMPARE(o.out.str(), "");
    }
};

SUBCHECK_OFFSCREEN_TEST_MAIN(SessionBridgeJobTest)
#include "session_bridge_job_test.moc"
