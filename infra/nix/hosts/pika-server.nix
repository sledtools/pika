import ../modules/pika-server.nix {
  hostname = "pika-server";
  domain = "api.pikachat.org";
  microvmSpawnerUrl = "http://100.81.250.67:8080";
}
