# dmgbuild settings for the disk image scripts/package.sh builds: the app beside a link to
# /Applications, on background.svg's spots, under Chauffeur's icon. dmgbuild writes the window's
# .DS_Store itself, so the build drives no Finder and runs headless.
#
#   dmgbuild -s scripts/dmg/settings.py -D app=<Chauffeur.app> -D icon=<icon.icns> \
#     -D background=scripts/dmg/background.png <volume name> <dmg>
import os.path

# dmgbuild runs this file with the -D values in `defines`.
app = defines["app"]  # noqa: F821

format = "UDZO"
filesystem = "HFS+"
files = [app]
symlinks = {"Applications": "/Applications"}
# Copied to .VolumeIcon.icns with the custom-icon flag: the mounted disk shows it on the Desktop
# and in the Finder's sidebar.
icon = defines["icon"]  # noqa: F821
# dmgbuild finds background@2x.png beside it and joins the two into one Retina TIFF.
background = defines["background"]  # noqa: F821

# The window's frame, title bar included: the background's bottom 28 points, empty ground, stay
# hidden.
window_rect = ((200, 120), (660, 400))
default_view = "icon-view"
show_status_bar = False
show_tab_view = False
show_toolbar = False
show_pathbar = False
show_sidebar = False
icon_size = 128
text_size = 13
icon_locations = {
    os.path.basename(app): (170, 200),
    "Applications": (490, 200),
}
