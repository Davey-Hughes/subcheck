# `cmake -D APP=<subcheck-qt> -P app_starts.cmake`, offscreen, in the smoke's store, not the
# user's: the app on a run that is not there. main.cpp's wiring holds if it says why on stderr and
# is still running when it is stopped, waiting on its dialog (window_test sees that dialog): a
# window that failed to load would have exited at once, saying nothing of the run.
if(NOT APP)
    message(FATAL_ERROR "app_starts.cmake: APP is not set")
endif()
execute_process(COMMAND "${APP}" no-such-run TIMEOUT 5 RESULT_VARIABLE status ERROR_VARIABLE err)
if(NOT status MATCHES "timeout")
    message(FATAL_ERROR "the app ended (${status}) before it was stopped:\n${err}")
endif()
if(NOT err MATCHES "subcheck-qt: no run named no-such-run\n")
    message(FATAL_ERROR "the app did not say why it cannot open the run:\n${err}")
endif()
