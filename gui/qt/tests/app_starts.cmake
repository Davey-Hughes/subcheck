# `cmake -D APP=<subcheck-qt> -D SAYS=<line> [-D ARGS=<arguments>] [-D DIR=<dir>] -P
# app_starts.cmake`, offscreen, in the store CTest names, not the user's: the app given ARGS, which
# name nothing it can open. main.cpp's wiring holds if it says why on stderr, in the line SAYS, and
# is still running when it is stopped, waiting on its dialog (window_test sees that dialog): a
# window that failed to load would have exited at once, saying nothing of the view. DIR, made if it
# is not there, holds a store of the test's own, so that the store's directory exists.
foreach(var APP SAYS)
    if(NOT ${var})
        message(FATAL_ERROR "app_starts.cmake: ${var} is not set")
    endif()
endforeach()
if(DIR)
    file(MAKE_DIRECTORY "${DIR}")
endif()
execute_process(COMMAND "${APP}" ${ARGS} TIMEOUT 5 RESULT_VARIABLE status ERROR_VARIABLE err)
if(NOT status MATCHES "timeout")
    message(FATAL_ERROR "the app ended (${status}) before it was stopped:\n${err}")
endif()
string(FIND "${err}" "${SAYS}\n" at)
if(at EQUAL -1)
    message(FATAL_ERROR "the app did not say why it cannot open the view:\n${err}")
endif()
