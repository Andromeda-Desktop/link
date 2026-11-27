project('andromeda-link-window', ['c', 'cpp'],
  meson_version: '>=1.1',
  default_options: [
    'cpp_std=c++23',
    'cpp_extensions=false',
    'warning_level=0',
    'b_ndebug=if-release',
  ],
)

# The Link window: a client of org.andromeda.Link1, linked with the static libdesk of the pinned desktop commit
# (dist/window/libdesk-commit; dist/window/build.sh builds it). One program on every distro, Andromeda too.
cpp = meson.get_compiler('cpp')
if cpp.get_define('_LIBCPP_VERSION', prefix: '#include <version>') != ''
  add_project_arguments('-fexperimental-library', language: 'cpp')
  add_project_link_arguments('-lc++experimental', language: 'cpp')
endif

_warnings = ['-Wall', '-Wextra', '-Wno-missing-field-initializers', '-Wpedantic', '-Wconversion', '-Wshadow']

_desk = dependency('andromeda-desk')
_translations = '-DANDROMEDA_TRANSLATIONS_DIR="' + meson.current_source_dir() / 'translations' + '"'
_av_deps = [dependency('libavcodec'), dependency('libavutil'), dependency('libavfilter'), dependency('libswscale'),
            dependency('libva')]

_model = static_library('link_model', 'src/link_model.cpp', dependencies: _desk, cpp_args: _warnings)

# Thousands of random steps per model: at -O0 the checks alone neared meson's 30 s timeout on a loaded machine.
test('link_model', executable('link_model_test', 'tests/link_model_test.cpp', link_with: _model, dependencies: _desk,
  include_directories: include_directories('src'), cpp_args: _warnings + [_translations],
  override_options: ['optimization=2']))

test('mirror_decoder', executable('mirror_decoder_test', 'tests/mirror_decoder_test.cpp', 'src/mirror_decoder.cpp',
  dependencies: [_desk] + _av_deps, include_directories: include_directories('src'),
  cpp_args: _warnings + ['-fsanitize=address', '-fno-omit-frame-pointer'], link_args: ['-fsanitize=address']),
  env: ['ASAN_OPTIONS=detect_leaks=0'])

# paths::assetsRoot finds <exe dir>/assets; /proc/self/exe resolves the bindir link to this directory.
_own_dir = get_option('prefix') / get_option('libdir') / 'andromeda-link'
executable('andromeda-link-app',
  files('src/main.cpp', 'src/app_main.cpp', 'src/link_app.cpp', 'src/link_client.cpp', 'src/mirror_decoder.cpp'),
  link_with: _model,
  dependencies: [_desk, dependency('libqrencode'), dependency('tomlplusplus').partial_dependency(includes: true)]
    + _av_deps,
  cpp_args: _warnings + [_translations],
  install: true,
  install_dir: _own_dir,
)
install_symlink('andromeda-link-app', pointing_to: _own_dir / 'andromeda-link-app', install_dir: get_option('bindir'))
install_subdir(_desk.get_variable(pkgconfig: 'assetsdir'), install_dir: _own_dir / 'assets', strip_directory: true)

install_data('translations/en.json', install_dir: get_option('datadir') / 'andromeda' / 'translations' / 'link')
install_data('data/dev.andromeda.Link.desktop', install_dir: get_option('datadir') / 'applications')
foreach size : ['16', '24', '32', '48', '64', '128', '256', '512']
  install_data('data/icons/hicolor' / size + 'x' + size / 'apps/dev.andromeda.Link.png',
               install_dir: get_option('datadir') / 'icons/hicolor' / size + 'x' + size / 'apps')
endforeach
# DBusActivatable=true in the .desktop file: the bus starts the app by its id, then calls Activate or Open.
configure_file(
  input: 'data/dev.andromeda.Link.service.in',
  output: 'dev.andromeda.Link.service',
  configuration: {'bindir': get_option('prefix') / get_option('bindir')},
  install_dir: get_option('datadir') / 'dbus-1' / 'services',
)
