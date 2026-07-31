set(
  CMAKE_MSVC_RUNTIME_LIBRARY
  "MultiThreaded$<$<CONFIG:Debug>:Debug>DLL"
  CACHE STRING
  "Use the dynamic MSVC runtime shared with Rust"
  FORCE
)
set(
  WITH_CRT_DLL
  ON
  CACHE BOOL
  "Build native dependencies against the dynamic MSVC runtime"
  FORCE
)
