from PIL import Image
from px import up
from monkey import draw_monkey, TAIL_A, TAIL_B, TAIL_LOW, ARM_L_REST, ARM_R_REST
import parts as P


# ---- arm poses -------------------------------------------------------------
def arms_up(c, hx, hy, ab):
    c.blit(P.ARM_L_OUT, 3, 18 + ab)
    c.blit(P.ARM_R_OUT, 21, 18 + ab)


def arms_flail(left_up):
    def f(c, hx, hy, ab):
        if left_up:
            c.blit(P.ARM_L_OUT, 3, 18 + ab)
            c.blit(P.ARM_R_SLAM, 16, 21 + ab)
        else:
            c.blit(P.ARM_L_SLAM, 10, 21 + ab)
            c.blit(P.ARM_R_OUT, 21, 18 + ab)
    return f


def arms_rub(dx):
    def f(c, hx, hy, ab):
        c.blit(ARM_L_REST, 10, 21 + ab)
        c.blit(P.ARM_R_SLAM, 16 + dx, 21 + ab)
    return f


def arms_wave(c, hx, hy, ab):
    c.blit(ARM_L_REST, 10, 21 + ab)
    c.blit(P.ARM_R_OUT, 21, 18 + ab)


def arms_chin(c, hx, hy, ab):
    c.blit(ARM_L_REST, 10, 21 + ab)
    c.blit(P.ARM_R_CHIN, 17, 18 + ab)


def arms_banana(c, hx, hy, ab):
    c.blit(P.BANANA2, 11, 21 + ab)
    c.blit(P.ARM_L_HOLD, 10, 20 + ab)
    c.blit(P.ARM_R_HOLD, 20, 20 + ab)


def arms_mug(c, hx, hy, ab):
    c.blit(P.MUG, 12, 21 + ab)
    c.blit(P.ARM_L_HOLD, 10, 21 + ab)
    c.blit(P.ARM_R_HOLD, 20, 21 + ab)


def arms_laptop(c, hx, hy, ab):
    c.blit(P.LAPTOP, 9, 20)
    c.blit(P.LAPTOP_BASE, 8, 27)


# ---- effects -----------------------------------------------------------------
def fx_at(sprite, x, y, outline=True):
    return lambda c, hx, hy: c.blit(sprite, x, y, outline=outline)


def fx_sweat(c, hx, hy):
    c.blit(P.SWEAT, hx + 17, hy + 1)


# ---- directional sheet (8 cols @ 32px -> 128px) ---------------------------
# Row layout must match the ROW_* constants in src/monkey.rs.
#   rows 0-7  gaze/travel sector: 0 right, 1 down-right, 2 down, 3 down-left,
#             4 left, 5 up-left, 6 up, 7 up-right.
#             cols 0-3 idle bob, cols 4-7 walk steps.
#   row 8     petted / purring loop
#   row 9     col 0 dragged (dangling), cols 1-7 thrown / tumbling flail
#   row 10    typing: cols 0-3 calm, cols 4-7 flushed (fast typing)
#   row 11    typing overheated: cols 0-3 steam A, cols 4-7 steam B
#             (typing cols: 0 paws ready, 1 left paw, 2 right paw, 3 both)
#   row 12    scroll snack: cols 0-5 banana peel stages, cols 6-7 eating
#   row 13    boop (clicked) reaction, played once
GAZE = [
    dict(face=(1, 0), eye_shift=(1, 0), head_dx=1),
    dict(face=(1, 1), eye_shift=(0, 0), head_dx=1),
    dict(face=(0, 1), eye_shift=(0, 0)),
    dict(face=(-1, 1), eye_shift=(0, 0), head_dx=-1),
    dict(face=(-1, 0), eye_shift=(-1, 0), head_dx=-1),
    dict(face=(-1, -1), eye_shift=(0, -1), head_dx=-1),
    dict(face=(0, -1), eye_shift=(0, -1)),
    dict(face=(1, -1), eye_shift=(0, -1), head_dx=1),
]

IDLE_WALK = [
    dict(bob=0, tail=TAIL_A),
    dict(bob=1, tail=TAIL_A),
    dict(bob=0, tail=TAIL_B),
    dict(bob=1, tail=TAIL_B),
    dict(bob=0, legs=(1, 0), tail=TAIL_A),
    dict(bob=1, legs=(0, 0), tail=TAIL_B),
    dict(bob=0, legs=(0, 1), tail=TAIL_A),
    dict(bob=1, legs=(0, 0), tail=TAIL_B),
]


def gaze_row(g):
    return [draw_monkey({**g, **f}) for f in IDLE_WALK]


def petted_row():
    frames = []
    sway = [0, 0, 1, 1, 0, 0, -1, -1]
    heart_y = [None, None, 9, 7, 5, 4, 3, 2]
    for i in range(8):
        fx = []
        if heart_y[i] is not None:
            spr = P.HEART_S if i < 4 else P.HEART
            fx.append(fx_at(spr, 24 + (i % 2), heart_y[i] - 1))
        frames.append(draw_monkey(dict(
            eyes='happy', mouth='smile', flush=1, head_dx=sway[i],
            bob=i // 2 % 2, tail=TAIL_A if i < 4 else TAIL_B, fx=fx,
        )))
    return frames


def dragged_tumble_row():
    frames = [draw_monkey(dict(
        eyes='wide', mouth='o', lift=2, arms=arms_up, fx=[fx_sweat], tail=TAIL_LOW,
    ))]
    for i in range(1, 8):
        left_up = i % 2 == 1
        frames.append(draw_monkey(dict(
            eyes='squeeze', mouth='yell', bob=i % 2,
            arms=arms_flail(left_up), tail=TAIL_LOW,
            fx=[fx_sweat] if i in (3, 4, 7) else [],
        )))
    return frames


# paw state: 0 both ready, 1 left down, 2 right down, 3 both down
def arms_type(paw):
    left_down = paw in (1, 3)
    right_down = paw in (2, 3)

    def f(c, hx, hy, ab):
        c.blit(P.KEYBOARD, 8, 25)
        # light up the key under each pressed paw
        if left_down:
            c.set(12, 26, 'c'); c.set(13, 26, 'c')
        if right_down:
            c.set(18, 26, 'c'); c.set(19, 26, 'c')
        if paw == 3:
            for x in range(11, 21):
                c.set(x, 28, 'c')
        # a pressed paw sits on the keys; a free paw winds up beside the shoulder
        # (both rest on the keyboard edge in the ready pose)
        if left_down:
            c.blit(P.TYPE_ARM_L, 10, 21)
            c.blit(P.PAW, 11, 25)
        elif paw == 0:
            c.blit(P.TYPE_ARM_L, 10, 21)
            c.blit(P.PAW, 10, 24)
        else:
            c.blit(P.PAW, 8, 20)
        if right_down:
            c.blit(P.TYPE_ARM_R, 20, 21)
            c.blit(P.PAW, 19, 25)
        elif paw == 0:
            c.blit(P.TYPE_ARM_R, 20, 21)
            c.blit(P.PAW, 20, 24)
        else:
            c.blit(P.PAW, 22, 20)
    return f


def fx_steam(phase):
    def f(c, hx, hy):
        if phase == 0:
            c.blit(P.PUFF, hx + 1, hy - 5)
            c.blit(P.PUFF_S, hx + 14, hy - 6)
        else:
            c.blit(P.PUFF, hx + 13, hy - 5)
            c.blit(P.PUFF_S, hx + 3, hy - 6)
    return f


def typing_rows():
    calm, warm, hot = [], [], []
    for paw in range(4):
        bob = 1 if paw else 0
        calm.append(draw_monkey(dict(
            eyes='open', mouth='flat' if paw == 0 else 'smile', face=(0, 1),
            arms=arms_type(paw), tail=TAIL_LOW, bob=bob,
        )))
        warm.append(draw_monkey(dict(
            eyes='open', mouth='o', face=(0, 1), flush=1,
            arms=arms_type(paw), tail=TAIL_LOW, bob=bob, fx=[fx_sweat],
        )))
    for phase in range(2):
        for paw in range(4):
            hot.append(draw_monkey(dict(
                eyes='squeeze', mouth='yell', face=(0, 1), flush=2,
                arms=arms_type(paw), tail=TAIL_LOW, bob=1 if paw else 0,
                fx=[fx_steam(phase)],
            )))
    return calm + warm, hot


def banana(peel, bite=0):
    """Upright banana held in both paws. `peel` 0-5 opens it from the top,
    `bite` removes rows of fruit from the top."""
    def f(c, hx, hy, ab):
        top, bottom = 15 + ab, 26 + ab
        px = {}
        px[(15, top - 1)] = 'n'
        px[(16, top - 1)] = 'n'
        line = top + peel * 2 if peel < 5 else top + 7
        for y in range(top + bite, bottom + 1):
            peeled = y < line
            for x in range(14, 18):
                if peeled:
                    px[(x, y)] = 'k' if x == 17 else 'h'
                else:
                    px[(x, y)] = 'Y' if x == 17 else 'y'
        if bite:
            px.pop((15, top - 1), None)
            px.pop((16, top - 1), None)
        if peel:
            # three flaps folding down from the peel line
            n = min(1 + peel, 4)
            for i in range(n):
                px[(13 - (i + 1) // 2, line + i)] = 'y'
                px[(18 + (i + 1) // 2, line + i)] = 'Y'
            for i in range(min(peel, 3)):
                px[(15, line + i)] = 'Y'
                px[(16, line + i)] = 'Y'
        xs = [p[0] for p in px]; ys = [p[1] for p in px]
        x0, y0 = min(xs), min(ys)
        rows = [''.join(px.get((x, y), '.') for x in range(x0, max(xs) + 1))
                for y in range(y0, max(ys) + 1)]
        c.blit(rows, x0, y0)
        c.blit(P.TYPE_ARM_L[:3], 10, 21 + ab)
        c.blit(P.PAW, 12, 23 + ab)
        c.blit(P.TYPE_ARM_R[:3], 20, 21 + ab)
        c.blit(P.PAW, 18, 23 + ab)
    return f


def snack_row():
    frames = []
    for stage in range(6):
        frames.append(draw_monkey(dict(
            eyes='open' if stage < 5 else 'happy', mouth='o',
            face=(0, -1) if stage < 5 else (0, 0),
            arms=banana(stage), tail=TAIL_A if stage % 2 else TAIL_B,
        )))
    frames.append(draw_monkey(dict(eyes='happy', mouth='open', flush=1, arms=banana(5, 3), tail=TAIL_B)))
    frames.append(draw_monkey(dict(eyes='happy', mouth='smile', flush=1, bob=1, arms=banana(5, 6), tail=TAIL_A)))
    return frames


def boop_row():
    spark = [fx_at(P.SPARKLE, 3, 6), fx_at(P.SPARKLE, 26, 4)]
    poses = [
        dict(eyes='squeeze', mouth='o', bob=1),
        dict(eyes='squeeze', mouth='open', bob=1),
        dict(eyes='happy', mouth='open', hop=2, arms=arms_up, tail=TAIL_LOW),
        dict(eyes='happy', mouth='open', hop=3, arms=arms_up, tail=TAIL_LOW, fx=spark),
        dict(eyes='happy', mouth='open', hop=2, arms=arms_up, tail=TAIL_LOW, fx=spark),
        dict(eyes='happy', mouth='smile', bob=1),
        dict(eyes='happy', mouth='smile'),
        dict(eyes='open', mouth='smile'),
    ]
    return [draw_monkey(p) for p in poses]


def build_directional():
    typing, hot = typing_rows()
    rows = [gaze_row(g) for g in GAZE] + [
        petted_row(), dragged_tumble_row(), typing, hot, snack_row(), boop_row(),
    ]
    sheet = Image.new('RGBA', (32 * 8, 32 * len(rows)), (0, 0, 0, 0))
    for r, frames in enumerate(rows):
        assert len(frames) == 8, (r, len(frames))
        for c, im in enumerate(frames):
            sheet.paste(im, (c * 32, r * 32), im)
    return sheet, rows


# ---- emote sheet (4x4 @ 32px -> 256px) --------------------------------------
EMOTES = [
    ('hi', dict(eyes='happy', mouth='open', arms=arms_wave, tail=TAIL_LOW)),
    ('lol', dict(eyes='squeeze', mouth='yell', fx=[fx_at(P.TEAR, 7, 13), fx_at(P.TEAR, 25, 13)])),
    ('love', dict(eyes='heart', mouth='smile', fx=[fx_at(P.HEART, 23, 2)])),
    ('sleepy', dict(eyes='blink', mouth='flat', blush=False, fx=[fx_at(P.Z_SMALL, 22, 5), fx_at(P.Z_BIG, 26, 0)])),
    ('sad', dict(eyes='open', mouth='frown', brows='sad', blush=False, fx=[fx_at(P.TEAR, 11, 16)])),
    ('mad', dict(eyes='open', mouth='frown', brows='angry', blush=False, fx=[fx_at(P.ANGER, 21, 4)])),
    ('shook', dict(eyes='wide', mouth='o', arms=arms_up, tail=TAIL_LOW, fx=[fx_sweat])),
    ('cool', dict(eyes='open', mouth='smile', blush=False, fx=[lambda c, hx, hy: c.blit(P.SHADES, hx + 3, hy + 5, outline=False)])),
    ('nom', dict(eyes='happy', mouth='open', arms=arms_banana)),
    ('coffee', dict(eyes='half', mouth='flat', blush=False, arms=arms_mug)),
    ('wfh', dict(eyes='open', mouth='flat', blush=False, face=(0, 1), arms=arms_laptop)),
    ('hmm', dict(eyes='open', mouth='flat', eye_shift=(1, -1), arms=arms_chin, fx=[fx_at(P.QMARK, 26, 1)])),
    ('wink', dict(eyes='wink', mouth='blep')),
    ('blep', dict(eyes='open', mouth='blep')),
    ('dizzy', dict(eyes='x', mouth='frown', blush=False, fx=[fx_at(P.SPARKLE, 4, 4), fx_at(P.SPARKLE, 25, 3)])),
    ('hype', dict(eyes='happy', mouth='yell', arms=arms_up, tail=TAIL_LOW, bob=1, fx=[fx_at(P.SPARKLE, 2, 6), fx_at(P.SPARKLE, 27, 6), fx_at(P.SPARKLE, 25, 0)])),
]


def build_emotes():
    sheet = Image.new('RGBA', (32 * 4, 32 * 4), (0, 0, 0, 0))
    ims = []
    for i, (name, pose) in enumerate(EMOTES):
        im = draw_monkey(pose)
        ims.append((name, im))
        r, c = divmod(i, 4)
        sheet.paste(im, (c * 32, r * 32), im)
    return sheet, ims


if __name__ == '__main__':
    import os
    out = os.path.join(os.path.dirname(os.path.abspath(__file__)), '..', '..', 'assets', 'sprites')
    directional, _ = build_directional()
    up(directional, 4).save(os.path.join(out, 'monkey_directional.png'), optimize=True)
    emotes, _ = build_emotes()
    up(emotes, 8).save(os.path.join(out, 'monkey_brown.png'), optimize=True)
