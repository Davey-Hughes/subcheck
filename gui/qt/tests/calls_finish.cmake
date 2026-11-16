# `cmake -D OBJECTS=<subcheck-qt's objects> -P calls_finish.cmake`: subcheck-qt's objects need
# the library's finish(), which nm names among the symbols they need from elsewhere, by its Itanium
# name (Mach-O's with an underscore more). That proves a call is there, not where. What nm lists
# must name main itself, so that a tool that printed nothing cannot pass for one that found none.
if(NOT OBJECTS)
    message(FATAL_ERROR "calls_finish.cmake: OBJECTS is not set")
endif()
execute_process(COMMAND nm ${OBJECTS} OUTPUT_VARIABLE symbols COMMAND_ERROR_IS_FATAL ANY)
if(NOT symbols MATCHES "T _?main\n")
    message(FATAL_ERROR "nm names no main in ${OBJECTS}:\n${symbols}")
endif()
if(NOT symbols MATCHES "U _?_ZN12subcheck_ffi6finishEv\n")
    message(FATAL_ERROR "main.cpp does not call subcheck_ffi::finish():\n${symbols}")
endif()
