#include "app/app_main.h"
#include "link_app.h"

int desk_app_main(int argc, char* argv[]) {
  if (const auto exit = DeskApp::forwardToRunning(linkapp::LinkApp::kAppId, argc, argv)) {
    return *exit;
  }
  linkapp::LinkApp app;
  return app.run(argc, argv);
}
