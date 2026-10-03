from px import Canvas, parse

HEAD = parse("""
|......LTLLFF......|
|....LLTLLFFFFF....|
|...LLLLFFFFFFFF...|
|..LLLFFFFFFFFFFD..|
|.LLFFFFFFFFFFFFFD.|
|.LFFFFFFFFFFFFFFD.|
|LFFFFFFFFFFFFFFFFD|
|FFFFFFFFFFFFFFFFFD|
|FFFFFFFFFFFFFFFFDD|
|DFFFFFFFFFFFFFFFDD|
|.DFFFFFFFFFFFFFDD.|
|.DDFFFFFFFFFFFFDD.|
|..DDDFFFFFFFFDDD..|
|....DDDDDDDDDD....|
""")

TUFT = parse("""
|.LF|
|LF.|
""")

FACE = parse("""
|..hkkk..kkkk..|
|.hhkkkkkkkkkk.|
|.hkkkkkkkkkkk.|
|hkkkkkkkkkkkks|
|hkkkkkkkkkkkks|
|kkkkkkkkkkkkks|
|.kkkkkkkkkkks.|
|.skkkkkkkkkks.|
|..sskkkkkkss..|
|....ssssss....|
""")

EAR_L = parse("""
|.LF.|
|Lkkf|
|Fksf|
|Dssf|
|.DD.|
""".replace('f', 'F'))

EAR_R = parse("""
|.FD.|
|FkkD|
|FskD|
|FssD|
|.DD.|
""")

BODY = parse("""
|..LFFFFD..|
|.LFFFFFFD.|
|LFFhkkkFFD|
|LFhkkkkkFD|
|FFhkkkkkFD|
|FFkkkkksFD|
|.DskkkssD.|
|..DDDDDD..|
""")

LEG_L = parse("""
|.FD|
|kks|
""")
LEG_R = parse("""
|FD.|
|kks|
""")

ARM_L_REST = parse("""
|.L|
|LF|
|LF|
|FD|
|hk|
|ks|
""")
ARM_R_REST = parse("""
|F.|
|FD|
|FD|
|FD|
|kk|
|ss|
""")

# Tail as a pixel path (frame coords relative to the tail anchor)
TAIL_A = (parse("""
|....LFF..|
|...LF.FD.|
|...F...FD|
|...FD..FD|
|....D.LF.|
|.....LF..|
|...LFD...|
|.LFFD....|
|FFD......|
"""), 0)
TAIL_LOW = (parse("""
|.....LFF.|
|....F...F|
|LFFFD..FD|
|.DDD..DD.|
"""), 5)
TAIL_B = (parse("""
|....LFF..|
|...LF.FD.|
|...F...FD|
|...FD..FD|
|....D.LF.|
|.....LF..|
|....LF...|
|..LFD....|
|LFFD.....|
|FD.......|
"""), -1)


EYES = {
    'open': ["we", "ee", "ee"],
    'blink': ["..", "..", "ee"],
    'half': ["..", "ss", "ee"],
}


def draw_tail(c, tail, ox, oy):
    rows, dy = tail
    c.blit(rows, ox, oy + dy)


def draw_face(c, hx, hy, pose):
    fdx, fdy = pose.get('face', (0, 0))
    ex, ey = pose.get('eye_shift', (0, 0))
    fx, fy = hx + 2 + fdx, hy + 3 + fdy
    head_px = {(x, y) for y in range(32) for x in range(32) if c.get(x, y) not in (None, 'O', 'I')}
    flush = pose.get('flush', 0)
    red = {'k': 'K', 'h': 'H', 's': 'S'} if flush >= 2 else {}
    for j, r in enumerate(FACE):
        for i, ch in enumerate(r):
            if ch != '.' and (fx + i, fy + j) in head_px:
                c.set(fx + i, fy + j, red.get(ch, ch))

    eyes = pose.get('eyes', 'open')
    ex0, ey0 = fx + 3 + ex, fy + 2 + ey
    if eyes in EYES:
        pat = EYES[eyes]
        for j, r in enumerate(pat):
            for i, ch in enumerate(r):
                if ch != '.':
                    c.set(ex0 + i, ey0 + j, ch)
                    # right eye (shine stays top-left for consistent light)
                    c.set(ex0 + 6 + i, ey0 + j, ch)
    elif eyes == 'happy':
        for (x, y) in [(0, 1), (1, 0), (2, 0), (3, 1)]:
            c.set(ex0 - 1 + x, ey0 + 1 + y, 'e')
            c.set(ex0 + 5 + x, ey0 + 1 + y, 'e')
    elif eyes == 'squeeze':  # > <
        for (x, y) in [(0, 0), (1, 1), (0, 2)]:
            c.set(ex0 + x, ey0 + y, 'e')
        for (x, y) in [(1, 0), (0, 1), (1, 2)]:
            c.set(ex0 + 6 + x, ey0 + y, 'e')
    elif eyes == 'wink':
        for j, r in enumerate(EYES['open']):
            for i, ch in enumerate(r):
                c.set(ex0 + i, ey0 + j, ch)
        for (x, y) in [(0, 1), (1, 0), (2, 0), (3, 1)]:
            c.set(ex0 + 5 + x, ey0 + 1 + y, 'e')
    elif eyes == 'heart':
        heart = ["q.q", "qqq", ".q."]
        for j, r in enumerate(heart):
            for i, ch in enumerate(r):
                if ch != '.':
                    c.set(ex0 - 1 + i, ey0 + j, ch)
                    c.set(ex0 + 5 + i, ey0 + j, ch)
        c.set(ex0 - 1, ey0, 'Q')
        c.set(ex0 + 5, ey0, 'Q')
    elif eyes == 'x':
        for (x, y) in [(0, 0), (2, 0), (1, 1), (0, 2), (2, 2)]:
            c.set(ex0 - 1 + x, ey0 + y, 'e')
            c.set(ex0 + 5 + x, ey0 + y, 'e')
    elif eyes == 'wide':
        pat = ["ww", "we", "ee"]
        for j, r in enumerate(pat):
            for i, ch in enumerate(r):
                c.set(ex0 + i, ey0 + j, 'e' if ch == 'e' else 'w')
                c.set(ex0 + 6 + i, ey0 + j, 'e' if ch == 'e' else 'w')
        # ring
        for dx, dy in [(-1, 0), (-1, 1), (-1, 2), (2, 0), (2, 1), (2, 2), (0, -1), (1, -1), (0, 3), (1, 3)]:
            c.set(ex0 + dx, ey0 + dy, 'e')
            c.set(ex0 + 6 + dx, ey0 + dy, 'e')

    if pose.get('brows') == 'angry':
        for (x, y) in [(2, 0), (3, 0), (4, 1)]:
            c.set(fx + x, fy + y + ey, 'e')
            c.set(fx + 13 - x, fy + y + ey, 'e')
    elif pose.get('brows') == 'sad':
        for (x, y) in [(2, 1), (3, 0), (4, 0)]:
            c.set(fx + x, fy + y + ey, 'e')
            c.set(fx + 13 - x, fy + y + ey, 'e')

    if pose.get('blush', True) or flush:
        for x in (1, 2):
            c.set(fx + x, fy + 5, 'p')
            c.set(fx + x + 10, fy + 5, 'p')
        if flush:
            for x in (1, 2, 3):
                c.set(fx + x, fy + 6, 'p')
                c.set(fx + x + 9, fy + 6, 'p')

    # nose
    c.set(fx + 6, fy + 5, 'n')
    c.set(fx + 7, fy + 5, 'n')

    mouth = pose.get('mouth', 'smile')
    mx, my = fx, fy
    M = {
        'smile': [(5, 7, 'm'), (8, 7, 'm'), (6, 8, 'm'), (7, 8, 'm')],
        'open': [(5, 7, 'm'), (6, 7, 'm'), (7, 7, 'm'), (8, 7, 'm'), (6, 8, 'r'), (7, 8, 'r')],
        'o': [(6, 7, 'm'), (7, 7, 'm'), (6, 8, 'm'), (7, 8, 'm')],
        'flat': [(6, 7, 'm'), (7, 7, 'm')],
        'frown': [(6, 7, 'm'), (7, 7, 'm'), (5, 8, 'm'), (8, 8, 'm')],
        'blep': [(5, 7, 'm'), (6, 7, 'm'), (7, 7, 'm'), (8, 7, 'm'), (7, 8, 'r'), (7, 9, 'r')],
        'yell': [(5, 7, 'm'), (6, 7, 'm'), (7, 7, 'm'), (8, 7, 'm'), (5, 8, 'm'), (6, 8, 'r'), (7, 8, 'r'), (8, 8, 'm')],
    }[mouth]
    for (x, y, ch) in M:
        c.set(mx + x, my + y, ch)


LEG_L_LONG = parse("""
|.FD|
|.FD|
|.FD|
|kks|
""")
LEG_R_LONG = parse("""
|FD.|
|FD.|
|FD.|
|kks|
""")


def draw_monkey(pose):
    c = Canvas()
    bob = pose.get('bob', 0)
    lift = pose.get('lift', 0)
    hx, hy = 7 + pose.get('head_dx', 0), 7 + bob + pose.get('head_dy', 0)

    c.oy = -lift
    draw_tail(c, pose.get('tail', TAIL_A), 21, 19 + pose.get('tail_dy', 0))

    c.oy = 0
    if lift:
        c.blit(LEG_L_LONG, 10, 26)
        c.blit(LEG_R_LONG, 19, 26)
    else:
        ll, rl = pose.get('legs', (0, 0))
        c.blit(LEG_L, 11, 28 - ll)
        c.blit(LEG_R, 18, 28 - rl)
    c.oy = -lift

    c.blit(BODY, 11, 20 + max(bob, 0))

    c.blit(EAR_L, hx - 3, hy + 5)
    c.blit(EAR_R, hx + 17, hy + 5)

    c.blit(HEAD, hx, hy)
    c.blit(TUFT, hx + 8, hy - 2)
    draw_face(c, hx, hy, pose)

    arms = pose.get('arms', 'rest')
    ab = max(bob, 0)
    if arms == 'rest':
        c.blit(ARM_L_REST, 10, 21 + ab)
        c.blit(ARM_R_REST, 20, 21 + ab)
    elif callable(arms):
        arms(c, hx, hy, ab)

    for fx in pose.get('fx', []):
        fx(c, hx, hy)
    im = c.image()
    hop = pose.get('hop', 0)
    if hop:
        shifted = im.crop((0, 0, im.width, im.height))
        shifted.paste((0, 0, 0, 0), (0, 0, im.width, im.height))
        shifted.paste(im.crop((0, hop, im.width, im.height)), (0, 0))
        im = shifted
    return im
