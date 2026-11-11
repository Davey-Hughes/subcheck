# `cmake -D APP=<subcheck-qt> -P one_binary.cmake`: the app links ffi/'s staticlib, so that it is
# one binary, and needs no library of ours at run time. What it needs must name Qt's core, so that
# a tool that printed nothing cannot pass for one that found nothing of ours.
if(NOT APP)
    message(FATAL_ERROR "one_binary.cmake: APP is not set")
endif()
if(APPLE)
    set(list_needed otool -L)
else()
    set(list_needed readelf -d)
endif()
execute_process(COMMAND ${list_needed} "${APP}" OUTPUT_VARIABLE needed COMMAND_ERROR_IS_FATAL ANY)
if(NOT needed MATCHES "Qt6?Core")
    message(FATAL_ERROR "${list_needed} names no Qt core in what ${APP} needs:\n${needed}")
endif()
if(needed MATCHES "subcheck_ffi")
    message(FATAL_ERROR "${APP} needs ffi/'s shared library at run time:\n${needed}")
endif()
