from PIL import Image

W = H = 32

PAL = {
    'O': (43, 26, 34),     # outline (dark plum)
    'I': (84, 44, 38),     # internal line
    'D': (118, 62, 42),    # fur shadow
    'F': (160, 90, 52),    # fur
    'L': (199, 124, 69),   # fur light
    'T': (226, 162, 98),   # fur highlight
    's': (214, 150, 115),  # skin shadow
    'k': (244, 201, 160),  # skin
    'h': (255, 231, 204),  # skin light
    'p': (242, 132, 125),  # blush
    'e': (43, 26, 34),     # eye / dark detail
    'w': (255, 255, 255),  # shine
    'm': (110, 40, 45),    # mouth dark
    'r': (222, 92, 98),    # tongue / mouth red
    'n': (92, 48, 40),     # nose
    'y': (250, 214, 84),   # banana
    'Y': (222, 164, 58),   # banana shadow
    'q': (255, 107, 139),  # heart
    'Q': (255, 186, 204),  # heart light
    'b': (124, 199, 242),  # tear/sweat
    'B': (214, 240, 255),  # tear light
    'a': (232, 74, 74),    # anger
    'z': (205, 214, 255),  # zzz
    'g': (184, 188, 204),  # laptop body
    'G': (110, 114, 136),  # laptop dark
    'c': (160, 232, 255),  # screen glow
    'u': (91, 158, 166),   # mug
    'U': (60, 110, 122),   # mug dark
    'v': (70, 42, 30),     # coffee
    'x': (236, 236, 244),  # steam / motion line
}


def parse(art):
    lines = [l for l in art.split('\n')]
    while lines and not lines[0].strip():
        lines.pop(0)
    while lines and not lines[-1].strip():
        lines.pop()
    # strip common leading indent (use '|' as optional left margin marker)
    rows = []
    for l in lines:
        l = l.strip()
        if l.startswith('|'):
            l = l[1:]
        if l.endswith('|'):
            l = l[:-1]
        rows.append(l)
    width = max(len(r) for r in rows)
    return [r.ljust(width, '.') for r in rows]


class Canvas:
    def __init__(self):
        self.px = [[None] * W for _ in range(H)]
        self.ox = 0
        self.oy = 0

    def get(self, x, y):
        x += self.ox
        y += self.oy
        if 0 <= x < W and 0 <= y < H:
            return self.px[y][x]
        return None

    def set(self, x, y, c):
        x += self.ox
        y += self.oy
        if 0 <= x < W and 0 <= y < H:
            self.px[y][x] = c

    def blit(self, rows, ox, oy, outline=True, inner=True):
        pts = {}
        for j, r in enumerate(rows):
            for i, ch in enumerate(r):
                if ch not in '. ':
                    pts[(ox + i, oy + j)] = ch
        if outline:
            ring = set()
            for (x, y) in pts:
                for dx, dy in ((1, 0), (-1, 0), (0, 1), (0, -1)):
                    n = (x + dx, y + dy)
                    if n not in pts:
                        ring.add(n)
            for (x, y) in ring:
                cur = self.get(x, y)
                if cur is None:
                    self.set(x, y, 'O')
                elif inner and cur not in ('O',):
                    self.set(x, y, 'I')
        for (x, y), ch in pts.items():
            self.set(x, y, ch)

    def dot(self, x, y, ch):
        self.set(x, y, ch)

    def image(self):
        im = Image.new('RGBA', (W, H), (0, 0, 0, 0))
        for y in range(H):
            for x in range(W):
                c = self.px[y][x]
                if c:
                    im.putpixel((x, y), PAL[c] + (255,))
        return im


def up(im, s):
    return im.resize((im.width * s, im.height * s), Image.NEAREST)
