# `cmake -D TEST=<program> [-D ARGS=<list>] <seed.cmake's -D options> -P seeded.cmake`: the run
# seed.cmake makes, made afresh in DIR, then TEST run on it with ARGS. For a test that decides
# findings, so that each run of it, as `ctest --repeat` makes, opens the run as the scan made it.
# seed.cmake removes DIR and makes it again, so CMakeLists.txt names it in the build directory.
if(NOT TEST)
    message(FATAL_ERROR "seeded.cmake: TEST is not set")
endif()
include("${CMAKE_CURRENT_LIST_DIR}/seed.cmake")
execute_process(COMMAND "${TEST}" ${ARGS} RESULT_VARIABLE status)
if(NOT status EQUAL 0)
    message(FATAL_ERROR "${TEST} ended with ${status}")
endif()
