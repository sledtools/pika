let
  prodAdmins = import ./prod-admins.nix;
in {
  inherit prodAdmins;
  prodAdminNpubs = map (admin: admin.npub) prodAdmins;
}
