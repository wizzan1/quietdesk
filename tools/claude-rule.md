## QuietDesk (`quietdesk`)

The user turns this on so test windows do not disturb them: `quietdesk on`, `quietdesk off`, `quietdesk status`. When they say "quiet desk on/off", "do not disturb on/off" or "hide/show the windows", run it. While it is on, what your commands open runs on a hidden desktop, with the real GPU at full speed, and nothing reaches their screen. They may be playing a game on this PC.

**It must never reduce what you test.** Test exactly as you would with it off. Never skip, shorten or weaken a check because of it.

- See, read and operate hidden windows with `quietdesk windows`, `quietdesk shot`, `quietdesk controls`, `quietdesk settext`, `quietdesk press`, `quietdesk keys`, `quietdesk click`, `quietdesk close`, and through the app's own channel (control pipe, console, debug port, its own screenshots).
- If anything behaves differently hidden (an app exits or refuses to start, a picture comes out black, a check measures the real display), rerun that one command with `quietdesk monitor <command>` and say so: a virtual monitor, out of sight, but to Windows a real display. A window can flash on the user's screen for up to 0.8 seconds as it opens, so hidden comes first. With the mode off it also plugs a monitor in and out, which freezes the user's screens for a second with the connect sound: never during their game.
- Only what needs the keyboard focus, or the user's eyes, goes on the real screen with `quietdesk visible <command>`. A test that needs the real screen wins over not disturbing.
- Known cases: Edge needs `quietdesk monitor`, and apps built on it may too. OpenGL windows capture black when hidden, so use their own screenshots or `quietdesk monitor`. A check of display timing is wrong while a full-screen game holds the main screen: rerun it when the game is closed, and report it as not run.
- A window you leave open for the user to try needs `quietdesk visible`, or they will never see it.
- A hidden dialog waiting for an answer looks like a hang: check with `quietdesk windows` and `quietdesk shot`.
- The `quietdesk` skill has the details.
