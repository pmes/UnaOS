# FONTBIDI (SR56) Chromium oracle job list: complex-script and mixed-direction paragraphs x sizes, each with a font
# stack (the script's Noto font, then DejaVu Sans for what it lacks — Chromium falls back per cluster, and so does
# font_core::shape_fallback). Usage: python3 oracle/fontbidi_jobs.py <noto-dir> > jobs.tsv
# Columns: id, comma-separated font paths, size px, text.
import sys, os
d = sys.argv[1]
DV = '/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf'
F = {k: os.path.join(d, v) for k, v in {
    'ar': 'NotoSansArabic-Regular.ttf', 'he': 'NotoSansHebrew-Regular.ttf',
    'de': 'NotoSansDevanagari-Regular.ttf', 'th': 'NotoSansThai-Regular.ttf', 'ur': 'NotoNastaliqUrdu-Regular.ttf'}.items()}
P = [
    ('ar1', 'ar', 'مرحبا بالعالم، هذه فقرة عربية للاختبار.'),
    ('ar2', 'ar', 'بِسْمِ اللَّهِ الرَّحْمَٰنِ الرَّحِيمِ'),
    ('ar3', 'ar', 'اللغة العربية جميلة جداً ولها تاريخ طويل'),
    ('ar4', 'ar', 'عام 2026 كان رائعاً، وعدد السكان ١٢٣٤٥٦.'),
    ('ar5', 'ar', 'لا إله إلا الله، لأن لإبراهيم لآلئ'),
    ('ur1', 'ur', 'اردو ایک خوبصورت زبان ہے'),
    ('ur2', 'ur', 'پاکستان کا دارالحکومت اسلام آباد ہے'),
    ('he1', 'he', 'שלום עולם, זוהי פסקה בעברית.'),
    ('he2', 'he', 'בְּרֵאשִׁית בָּרָא אֱלֹהִים אֵת הַשָּׁמַיִם'),
    ('he3', 'he', 'הספר "Unicode 17" יצא ב-2025.'),
    ('de1', 'de', 'नमस्ते दुनिया, यह हिन्दी में एक अनुच्छेद है।'),
    ('de2', 'de', 'संस्कृत भाषा विश्व की प्राचीनतम भाषाओं में से एक है।'),
    ('de3', 'de', 'क्षत्रिय, धर्म, कार्य, प्रेम, श्री, राष्ट्र, स्त्री'),
    ('de4', 'de', 'उन्होंने कहा कि ज़िंदगी पढ़ाई से बनती है'),
    ('th1', 'th', 'สวัสดีชาวโลก ภาษาไทยเป็นภาษาที่สวยงาม'),
    ('th2', 'th', 'น้ำใจ ผู้ใหญ่ ญี่ปุ่น กำลังใจ ฤๅษี'),
    ('mx1', 'ar', 'The word مرحبا means hello.'),
    ('mx2', 'he', 'Hebrew שלום עולם and English together.'),
    ('mx3', 'ar', 'في عام 2026 قال: "Hello World" ثم غادر.'),
    ('mx4', 'he', 'abc (שלום [עולם] 123) def'),
    ('mx5', 'de', 'हिन्दी and English mixed, 2026.'),
    ('mx6', 'th', 'ภาษาไทย Thai 123 ไทย'),
    ('mx7', 'ar', 'Version 2.0 من البرنامج (beta) جاهز!'),
    ('mx8', 'he', 'עברית English עברית 42 English'),
]
SIZES = [12, 16, 24, 48]
for pid, f, text in P:
    for z in SIZES:
        print(f'{pid}_{z}\t{F[f]},{DV}\t{z}\t{text}')
