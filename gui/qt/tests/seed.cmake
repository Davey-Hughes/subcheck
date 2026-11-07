# `cmake -D SUBCHECK=<subcheck> -D REPO_ROOT=<repo> -D DIR=<dir> -P seed.cmake`: a store in DIR
# holding one run, made by the real `subcheck scan` over a copy of typos.mkv in DIR/media, with
# core's fake LLM. Made afresh each time, so that every test run opens the same run.
# Each must be set: with DIR empty, what follows would work at the filesystem's root.
foreach(var SUBCHECK REPO_ROOT DIR)
    if(NOT ${var})
        message(FATAL_ERROR "seed.cmake: ${var} is not set")
    endif()
endforeach()
file(REMOVE_RECURSE "${DIR}")
file(MAKE_DIRECTORY "${DIR}/media" "${DIR}/state")
file(COPY "${REPO_ROOT}/core/tests/fixtures/typos.mkv" DESTINATION "${DIR}/media")
execute_process(
    COMMAND "${CMAKE_COMMAND}" -E env "SUBCHECK_DB=${DIR}/subcheck.db" "XDG_STATE_HOME=${DIR}/state"
            "${SUBCHECK}" scan --no-tui --llm-cmd "'${REPO_ROOT}/core/tests/fake-llm.sh'"
            "${DIR}/media"
    COMMAND_ERROR_IS_FATAL ANY)
