cmake_minimum_required(VERSION 3.14)
project(BosphorusUiPlugin LANGUAGES CXX)

if(DEFINED ENV{LOGOS_MODULE_BUILDER_ROOT})
    include($ENV{LOGOS_MODULE_BUILDER_ROOT}/cmake/LogosModule.cmake)
else()
    message(FATAL_ERROR "LogosModule.cmake not found. Set LOGOS_MODULE_BUILDER_ROOT.")
endif()

logos_module(
    NAME bosphorus_ui
    REP_FILE src/bosphorus_backend.rep
    SOURCES
        src/bosphorus_backend.h
        src/bosphorus_backend.cpp
    INCLUDE_DIRS
        src
)
