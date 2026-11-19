# `cmake -D TEST=<program> -P refuses.cmake`: the test must refuse to start, with 2 and its reason,
# before it makes anything a desktop could show: on a platform that is not offscreen, and on
# offscreen with a display to open. No server listens on the displays named, so that a test that
# does start draws nowhere.
if(NOT TEST)
    message(FATAL_ERROR "refuses.cmake: TEST is not set")
endif()
foreach(desktop IN ITEMS "QT_QPA_PLATFORM=minimal" "DISPLAY=:65000"
                         "WAYLAND_DISPLAY=subcheck-no-such-display")
    execute_process(COMMAND "${CMAKE_COMMAND}" -E env "${desktop}" "${TEST}"
                    RESULT_VARIABLE status ERROR_VARIABLE err)
    if(NOT status EQUAL 2 OR NOT err MATCHES "run through ctest")
        message(FATAL_ERROR "should have refused to start, but started (${status}) with "
                            "${desktop}: ${TEST}\n${err}")
    endif()
endforeach()
