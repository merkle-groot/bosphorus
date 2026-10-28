// mouse click X Y | mouse type "text" | mouse key CODE  — real HID-level events (points).
#include <ApplicationServices/ApplicationServices.h>
#include <string.h>
#include <stdlib.h>
#include <unistd.h>
// Every event carries explicit modifier flags (none, or Command for `key CODE cmd`): an event
// created without them inherits the system's current state, which is how a ⌘A once left Command
// held for the text typed after it (docs/spikes/B3-basecamp-module.md).
static CGEventFlags flags = 0;
static void post(CGEventRef e) { CGEventSetFlags(e, flags); CGEventPost(kCGHIDEventTap, e); CFRelease(e); usleep(20000); }
int main(int argc, char** argv) {
  if (argc >= 4 && !strcmp(argv[1], "click")) {
    CGPoint p = CGPointMake(atof(argv[2]), atof(argv[3]));
    post(CGEventCreateMouseEvent(NULL, kCGEventMouseMoved, p, kCGMouseButtonLeft));
    usleep(100000);
    post(CGEventCreateMouseEvent(NULL, kCGEventLeftMouseDown, p, kCGMouseButtonLeft));
    usleep(60000);
    post(CGEventCreateMouseEvent(NULL, kCGEventLeftMouseUp, p, kCGMouseButtonLeft));
  } else if (argc >= 3 && !strcmp(argv[1], "type")) {
    const char* s = argv[2];
    for (size_t i = 0; s[i]; i++) {
      UniChar c = (unsigned char)s[i];
      CGEventRef d = CGEventCreateKeyboardEvent(NULL, 0, true); CGEventKeyboardSetUnicodeString(d, 1, &c); post(d);
      CGEventRef u = CGEventCreateKeyboardEvent(NULL, 0, false); CGEventKeyboardSetUnicodeString(u, 1, &c); post(u);
    }
  } else if (argc >= 3 && !strcmp(argv[1], "key")) {
    CGKeyCode k = (CGKeyCode)atoi(argv[2]);
    CGEventRef d = CGEventCreateKeyboardEvent(NULL, k, true), u = CGEventCreateKeyboardEvent(NULL, k, false);
    if (argc >= 4 && !strcmp(argv[3], "cmd")) flags = kCGEventFlagMaskCommand;
    post(d); post(u);
    flags = 0;
    post(CGEventCreateKeyboardEvent(NULL, 55, false)); // Command up, flags clear
  } else if (argc >= 3 && !strcmp(argv[1], "scroll")) {
    post(CGEventCreateScrollWheelEvent(NULL, kCGScrollEventUnitLine, 1, atoi(argv[2])));
  } else return 1;
  return 0;
}
