# `cmake -D SUBCHECK=<subcheck> -D REPO_ROOT=<repo> -D DIR=<dir> [-D FIXTURE=<name>]
# [-D COPIES=<n>] [-D CLEAN=1] -P seed.cmake`: a store in DIR holding one run, made by the real
# `subcheck scan` over core's fixture FIXTURE (typos.mkv unless named) copied into DIR/media, with
# core's fake LLM. Made afresh each time, so that every test run opens the same run. Two copies or
# more are named as a show's episodes, whose like findings group into rows of a member per copy;
# one copy keeps the fixture's name. CLEAN adds a second run to the store: a copy of typos.mkv in
# DIR/clean that the fake LLM finds nothing in, so that its folder is a view with no findings.
# SUBCHECK, REPO_ROOT and DIR must be set: with DIR empty, what follows would work at the
# filesystem's root.
foreach(var SUBCHECK REPO_ROOT DIR)
    if(NOT ${var})
        message(FATAL_ERROR "seed.cmake: ${var} is not set")
    endif()
endforeach()
# On a filesystem that ignores case, a directory whose name differs from the executable's only by
# case is the same path, and running it fails with no more than "permission denied": this says
# what happened, and what to do.
if(IS_DIRECTORY "${SUBCHECK}")
    message(FATAL_ERROR "seed.cmake: ${SUBCHECK} is a directory, not subcheck: on a filesystem "
        "that ignores case, something in the build directory takes its name in another case. "
        "Remove the build directory and build again.")
endif()
file(REMOVE_RECURSE "${DIR}")
file(MAKE_DIRECTORY "${DIR}/media" "${DIR}/state")
if(NOT FIXTURE)
    set(FIXTURE typos.mkv)
endif()
set(fixture "${REPO_ROOT}/core/tests/fixtures/${FIXTURE}")
get_filename_component(show "${FIXTURE}" NAME_WLE)
if(NOT COPIES OR COPIES EQUAL 1)
    file(COPY "${fixture}" DESTINATION "${DIR}/media")
else()
    foreach(n RANGE 1 ${COPIES})
        if(n LESS 10)
            set(n "0${n}")
        endif()
        file(COPY_FILE "${fixture}" "${DIR}/media/${show} - E${n}.mkv")
    endforeach()
endif()
execute_process(
    COMMAND "${CMAKE_COMMAND}" -E env "SUBCHECK_DB=${DIR}/subcheck.db" "XDG_STATE_HOME=${DIR}/state"
            "${SUBCHECK}" scan --no-tui --llm-cmd "'${REPO_ROOT}/core/tests/fake-llm.sh'"
            "${DIR}/media"
    COMMAND_ERROR_IS_FATAL ANY)
if(CLEAN)
    file(COPY "${REPO_ROOT}/core/tests/fixtures/typos.mkv" DESTINATION "${DIR}/clean")
    execute_process(
        COMMAND "${CMAKE_COMMAND}" -E env "SUBCHECK_DB=${DIR}/subcheck.db"
                "XDG_STATE_HOME=${DIR}/state" FAKE_LLM_REPLY=NONE
                "${SUBCHECK}" scan --no-tui --llm-cmd "'${REPO_ROOT}/core/tests/fake-llm.sh'"
                "${DIR}/clean"
        COMMAND_ERROR_IS_FATAL ANY)
endif()
