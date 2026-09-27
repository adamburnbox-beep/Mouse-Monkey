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


# ---- directional sheet (8x8 @ 32px -> 128px) ------------------------------
# row = gaze sector from src/monkey.rs: 0 right, 1 down-right, 2 down,
# 3 down-left, 4 left, 5 up-left (idle clamps up/up-right to 5).
GAZE = [
    dict(face=(1, 0), eye_shift=(1, 0), head_dx=1),
    dict(face=(1, 1), eye_shift=(0, 0), head_dx=1),
    dict(face=(0, 1), eye_shift=(0, 0)),
    dict(face=(-1, 1), eye_shift=(0, 0), head_dx=-1),
    dict(face=(-1, 0), eye_shift=(-1, 0), head_dx=-1),
    dict(face=(-1, -1), eye_shift=(0, -1), head_dx=-1),
]

# cols 0-3: idle breathing (Idle @2Hz uses these); cols 4-7: walk steps
# (Hunting cycles all eight @8Hz).
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


def groom_row():
    frames = []
    heart_y = [None, None, 9, 7, 5, 4, 3, 2]
    for i in range(8):
        fx = []
        hyy = heart_y[i]
        if hyy is not None:
            spr = P.HEART_S if i < 4 else P.HEART
            fx.append(fx_at(spr, 24 + (i % 2), hyy - 1))
        frames.append(draw_monkey(dict(
            eyes='happy', mouth='smile', bob=i // 2 % 2,
            arms=arms_rub(-1 if i % 2 else 0),
            tail=TAIL_A if i < 4 else TAIL_B, fx=fx,
        )))
    return frames


def action_row():
    # col 0 = Dragged (static): lifted, arms up, legs dangling, shocked.
    frames = [draw_monkey(dict(
        eyes='wide', mouth='o', lift=2, arms=arms_up, fx=[fx_sweat], tail=TAIL_LOW,
    ))]
    # cols 1-7: alternating arm flail, used for key-mashing and tumbling.
    for i in range(1, 8):
        left_up = i % 2 == 1
        frames.append(draw_monkey(dict(
            eyes='squeeze', mouth='yell', bob=i % 2,
            arms=arms_flail(left_up),
            tail=TAIL_LOW,
            fx=[fx_sweat] if i in (3, 4, 7) else [],
        )))
    return frames


def build_directional():
    rows = [gaze_row(g) for g in GAZE] + [groom_row(), action_row()]
    sheet = Image.new('RGBA', (32 * 8, 32 * 8), (0, 0, 0, 0))
    for r, frames in enumerate(rows):
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
