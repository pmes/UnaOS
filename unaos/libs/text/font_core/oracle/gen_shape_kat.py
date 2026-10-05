# FONTBIDI (SR56) glyph-level oracle: HarfBuzz (uharfbuzz, test-time only — the engine inside Chromium) shapes the
# same strings with the same fonts; writes tests/data/hb_shape_kat.tsv. Fonts are identified by sha256 and fetched
# by the test (oracle/vectors.txt); a missing/different font is skipped by name.
# Usage: python3 oracle/gen_shape_kat.py <fonts-dir> > tests/data/hb_shape_kat.tsv
import sys, os, hashlib
import uharfbuzz as hb

FONTS = {
    'NotoSansArabic-Regular.ttf': 'Arab',
    'NotoSansHebrew-Regular.ttf': 'Hebr',
    'NotoSansDevanagari-Regular.ttf': 'Deva',
    'NotoSansThai-Regular.ttf': 'Thai',
    'NotoNastaliqUrdu-Regular.ttf': 'Urdu',
    'fontbidi_synth.ttf': 'Latn',
    'DejaVuSans.ttf': 'LatnDV',
}
TEXTS = {
    'Arab': ["مرحبا بالعالم", "السلام عليكم", "لا إله إلا الله", "بِسْمِ اللَّهِ الرَّحْمَٰنِ الرَّحِيمِ", "كتـــاب",
             "العربية لغة جميلة", "فِي", "عَلَى", "ـبـ", "لله", "مستشفى", "يتعلّمون", "سنة ٢٠٢٦", "إِنَّ", "ﻻ",
             "قُلْ هُوَ اللَّهُ أَحَدٌ", "الْحَمْدُ لِلَّهِ رَبِّ الْعَالَمِينَ", "ئ ؤ ة ى", "پچژگ", "لأ لإ لآ",
             "ب‍ب", "ب‌ب", "محمد", "شكراً جزيلاً", "تَّ", "أهلاً وسهلاً"],
    'Hebr': ["שלום עולם", "בְּרֵאשִׁית בָּרָא אֱלֹהִים", "עִבְרִית", "שָׁלוֹם", "יְרוּשָׁלַיִם", "ספר", "תּוֹרָה",
             "וַיֹּאמֶר", "כָּל־הָאָרֶץ", "שׁ שׂ", "אֲנִי", "מִצְוָה", "ךםןףץ", "הַשָּׁמַיִם"],
    'Deva': ["नमस्ते", "हिन्दी", "क्षत्रिय", "धर्म", "कार्य", "प्रेम", "द्वार", "श्री", "विद्यालय", "राष्ट्र",
             "किताब", "र्क", "कि", "क्कि", "स्त्री", "ज़िंदगी", "ॐ", "पढ़ाई", "उन्होंने", "अर्थात्", "हृदय", "र्कि",
             "ट्र", "क्‍ष", "क्‌ष", "आत्मा", "संस्कृत", "भारत गणराज्य", "दुर्गा", "पृथ्वी", "ध्यान", "ङ्क", "द्ध", "त्र",
             "र्त्स्न्य", "कर्ता", "ि", "कृ", "र्य", "ऋषि"],
    'Urdu': ["اردو زبان", "پاکستان", "نستعلیق", "خوش آمدید", "محبت", "کتاب", "میں ٹھیک ہوں", "شکریہ", "بہت", "لاہور",
             "سچ", "نبی", "تمہیں", "گھر", "ہمیشہ", "پنجاب", "بِسْمِ", "عِلْم", "یہ ایک جملہ ہے"],
    'Latn': ["xoxo", "aob eod", "qaqa", "ozz", "mmmn", "mnw", "TAVA AVW", "kl kl", "iii ij", "aobd", "xyz mnmnw",
             "AV AVW VAW", "Tea", "kolo", "aeob"],
    'LatnDV': ["Hello, world!", "office affluent fjord", "Ŵ̃ q̣̂ ệ", "e\u0301 a\u0323\u0302 x\u0302\u0308", "Ελληνικά",
               "Кириллица ё й", "AVATAR Toyota", "ﬁ ﬂ", "i\u0307\u0301", "Z\u0335\u0327"],
    'Thai': ["สวัสดี", "ภาษาไทย", "น้ำ", "ที่", "กำลัง", "ปู่", "ฤๅษี", "เป็น", "ผู้ใหญ่", "ญี่ปุ่น", "ขอบคุณครับ",
             "ทำ", "ป่ำ", "ฐูญุ", "กิ่ง", "สิ้น", "ปั้น", "ตั๋ว"],
}
import random
rng = random.Random(int(os.environ.get('FONTBIDI_SEED', '56')))
SCALE = int(os.environ.get('FONTBIDI_SCALE', '1'))
# Random strings over each script's letters and marks: broken clusters, stacked marks, reph/half/matra mixes.
POOLS = {
    'Arab': [chr(c) for c in list(range(0x0621, 0x063B)) + list(range(0x0641, 0x064B)) + list(range(0x064B, 0x0653)) + [0x0670, 0x0640, 0x067E, 0x0686, 0x06A9, 0x06AF, 0x06CC, 0x200C, 0x200D, 0x0660, 0x0661, 0x0020]],
    'Hebr': [chr(c) for c in list(range(0x05D0, 0x05EB)) + list(range(0x05B0, 0x05BE)) + [0x05BF, 0x05C1, 0x05C2, 0x05C7, 0x0591, 0x05A5, 0x05BE, 0x0020]],
    'Deva': [chr(c) for c in list(range(0x0915, 0x093A)) + list(range(0x093E, 0x094E)) + [0x0901, 0x0902, 0x0903, 0x093C, 0x094D, 0x094D, 0x094D, 0x0930, 0x0930, 0x0905, 0x0906, 0x0907, 0x0909, 0x090F, 0x200C, 0x200D, 0x0964, 0x0020]],
    'Urdu': [chr(c) for c in list(range(0x0627, 0x063B)) + list(range(0x0641, 0x064B)) + [0x064E, 0x064F, 0x0650, 0x0651, 0x0652, 0x0679, 0x067E, 0x0686, 0x0688, 0x0691, 0x0698, 0x06A9, 0x06AF, 0x06BA, 0x06BE, 0x06C1, 0x06CC, 0x06D2, 0x0020]],
    'Latn': [chr(c) for c in b'aeobdixyzqmnwklijTAVW '],
    'LatnDV': [chr(c) for c in list(b'aeoAVTWfily ') + list(range(0x0300, 0x0316)) + [0x0323, 0x0327, 0x0328, 0x0331, 0x03B1, 0x0430]],
    'Thai': [chr(c) for c in list(range(0x0E01, 0x0E2F)) + list(range(0x0E30, 0x0E3B)) + list(range(0x0E40, 0x0E4F)) + [0x0020]],
}
for sc, pool in POOLS.items():
    for _ in range((135 if sc not in ('Urdu', 'Latn', 'LatnDV') else 65) * SCALE):
        TEXTS[sc].append(''.join(rng.choice(pool) for _ in range(rng.randint(2, 9))).strip() or pool[0])
d = sys.argv[1]
print("# HarfBuzz %s (uharfbuzz %s) shaping KATs: font\tsha256\tscript\tdirection\ttext(hex)\tglyph,cluster,x_advance,x_offset,y_offset;..." % (hb.version_string(), hb.__version__))
for fn, sc in FONTS.items():
    path = os.path.join(d, fn)
    if fn.startswith('fontbidi_'):
        path = os.path.join(os.path.dirname(__file__), '..', 'tests', 'data', fn)
    if fn.startswith('DejaVu'):
        path = os.path.join('/usr/share/fonts/truetype/dejavu', fn)
    data = open(path, 'rb').read()
    sha = hashlib.sha256(data).hexdigest()
    face = hb.Face(data)
    font = hb.Font(face)
    for t in TEXTS[sc]:
        buf = hb.Buffer()
        buf.add_str(t)
        buf.guess_segment_properties()
        hb.shape(font, buf)
        # clusters are code point indices; convert to UTF-8 byte offsets
        offs = []
        b = 0
        for ch in t:
            offs.append(b)
            b += len(ch.encode('utf-8'))
        gl = ';'.join('%d,%d,%d,%d,%d' % (i.codepoint, offs[i.cluster], p.x_advance, p.x_offset, p.y_offset)
                      for i, p in zip(buf.glyph_infos, buf.glyph_positions))
        tag = buf.script or 'Zyyy'  # the script HarfBuzz itemized (Common for a lone tatweel), as Chromium would pass it
        print('\t'.join([fn, sha, tag, buf.direction, ' '.join('%04X' % ord(c) for c in t), gl]))
