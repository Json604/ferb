// FERB.app (spike 5): a container only — no windows, menus or Dock icon (CLAUDE.md: no graphical UI).
// Its sole job is to carry the Safari web extension; launching it once lets Safari discover the extension.
// (File keeps the template's name to avoid project surgery.)

@main
enum Container {
    static func main() {}
}
