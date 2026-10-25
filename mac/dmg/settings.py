# Layout of Radlet's disk image window, for dmgbuild. make-dmg.sh runs it
# with -D app=path/to/Radlet.app -D dir=path/to/this/folder. The background
# is drawn for these positions: Radlet on the left, an arrow, Applications
# on the right.
import os

app = defines["app"]  # noqa: F821 (dmgbuild provides `defines`)
here = defines.get("dir", "mac/dmg")  # noqa: F821

format = "UDZO"
files = [app]
symlinks = {"Applications": "/Applications"}
hide_extension = [os.path.basename(app)]

# The mounted disk shows Radlet's icon.
icon = os.path.join(app, "Contents", "Resources", "AppIcon.icns")

# background.png plus background@2x.png become one Retina background.
background = os.path.join(here, "background.png")
window_rect = ((200, 140), (660, 400))
default_view = "icon-view"
show_status_bar = False
show_tab_view = False
show_toolbar = False
show_pathbar = False
show_sidebar = False
icon_size = 112
text_size = 13
icon_locations = {
    os.path.basename(app): (190, 210),
    "Applications": (470, 210),
}
