"""Personal identifiers for the secret-shape corpus (imported by make.py; NOT Python-recorded). Every value is invented
here from the seeded random source and made checksum-valid by these implementations of each issuer's check (written
apart from the Rust ones in ferb-sanitize/src/pii.rs, so a misread algorithm shows as a failing test). No value is a
real person's: the digits are random, and a random valid number is assigned to someone only by chance (the corpus
never says whose; the commit gate's personal-data list has none of them)."""

import random
import string

UP = string.ascii_uppercase
DG = string.digits


def verhoeff_check(num: str) -> str:
    d = [[0, 1, 2, 3, 4, 5, 6, 7, 8, 9], [1, 2, 3, 4, 0, 6, 7, 8, 9, 5], [2, 3, 4, 0, 1, 7, 8, 9, 5, 6],
         [3, 4, 0, 1, 2, 8, 9, 5, 6, 7], [4, 0, 1, 2, 3, 9, 5, 6, 7, 8], [5, 9, 8, 7, 6, 0, 4, 3, 2, 1],
         [6, 5, 9, 8, 7, 1, 0, 4, 3, 2], [7, 6, 5, 9, 8, 2, 1, 0, 4, 3], [8, 7, 6, 5, 9, 3, 2, 1, 0, 4],
         [9, 8, 7, 6, 5, 4, 3, 2, 1, 0]]
    p = [[0, 1, 2, 3, 4, 5, 6, 7, 8, 9], [1, 5, 7, 6, 2, 8, 3, 0, 9, 4], [5, 8, 0, 3, 7, 9, 6, 1, 4, 2],
         [8, 9, 1, 6, 0, 4, 3, 5, 2, 7], [9, 4, 5, 3, 1, 2, 6, 8, 7, 0], [4, 2, 8, 6, 5, 7, 3, 9, 0, 1],
         [2, 7, 9, 3, 8, 0, 6, 4, 1, 5], [7, 0, 4, 6, 9, 1, 3, 2, 5, 8]]
    inv = [0, 4, 3, 2, 1, 5, 6, 7, 8, 9]
    c = 0
    for i, ch in enumerate(reversed(num)):
        c = d[c][p[(i + 1) % 8][int(ch)]]
    return str(inv[c])


def luhn_check(num: str) -> str:
    total = 0
    for i, ch in enumerate(reversed(num)):
        x = int(ch)
        if i % 2 == 0:
            x *= 2
            x = x - 9 if x > 9 else x
        total += x
    return str((10 - total % 10) % 10)


class Gen:
    def __init__(self, rng: random.Random):
        self.rng = rng

    def d(self, n: int) -> str:
        return "".join(self.rng.choice(DG) for _ in range(n))

    def u(self, n: int, alphabet: str = UP) -> str:
        return "".join(self.rng.choice(alphabet) for _ in range(n))

    def date6(self, order: str = "ymd") -> str:
        y, m, dd = self.rng.randint(50, 99), self.rng.randint(1, 12), self.rng.randint(1, 28)
        parts = {"y": f"{y:02d}", "m": f"{m:02d}", "d": f"{dd:02d}"}
        return "".join(parts[c] for c in order)

    def aadhaar(self) -> str:
        body = str(self.rng.randint(2, 9)) + self.d(10)
        n = body + verhoeff_check(body)
        return f"{n[:4]} {n[4:8]} {n[8:]}"

    def aadhaar_plain(self) -> str:
        return self.aadhaar().replace(" ", "")

    def aadhaar_bad(self) -> str:
        n = self.aadhaar_plain()
        wrong = str((int(n[-1]) + 1) % 10)
        n = n[:-1] + wrong
        return f"{n[:4]} {n[4:8]} {n[8:]}"

    def vid(self) -> str:
        body = self.d(15)
        n = body + verhoeff_check(body)
        return f"{n[:4]} {n[4:8]} {n[8:12]} {n[12:]}"

    def pan(self) -> str:
        return self.u(3) + self.rng.choice("PCHFATBLJG") + self.u(1) + self.d(4) + self.u(1)

    def epic(self) -> str:
        # Luhn over the seven digits (python-stdnum's in_.epic)
        body = self.d(6)
        return self.u(3) + body + luhn_check(body)

    def ssn(self) -> str:
        while True:
            a, g, s = self.rng.randint(1, 899), self.rng.randint(1, 99), self.rng.randint(1, 9999)
            if a != 666:
                return f"{a:03d}-{g:02d}-{s:04d}"

    def itin(self) -> str:
        g = self.rng.choice([*range(50, 66), *range(70, 89), *range(90, 93), *range(94, 100)])
        return f"9{self.d(2)}-{g:02d}-{self.d(4)}"

    def mbi(self) -> str:
        letters = "".join(c for c in UP if c not in "SLOIBZ")
        an = letters + DG
        r = self.rng
        return (str(r.randint(1, 9)) + r.choice(letters) + r.choice(an) + r.choice(DG) + "-" + r.choice(letters)
                + r.choice(an) + r.choice(DG) + "-" + r.choice(letters) + r.choice(letters) + r.choice(DG) + r.choice(DG))

    def routing(self) -> str:
        while True:
            body = self.d(8)
            for last in DG:
                n = body + last
                if sum(int(c) * w for c, w in zip(n, [3, 7, 1] * 3)) % 10 == 0:
                    return n

    def nino(self) -> str:
        while True:
            p, q = self.rng.choice(UP), self.rng.choice(UP)
            if p in "DFIQUV" or q in "DFIOQUV" or p + q in ("BG", "GB", "NK", "KN", "TN", "NT", "ZZ"):
                continue
            n = self.d(6)
            return f"{p}{q} {n[:2]} {n[2:4]} {n[4:]} {self.rng.choice('ABCD')}"

    def nhs(self) -> str:
        while True:
            body = self.d(9)
            r = 11 - sum(int(c) * w for c, w in zip(body, range(10, 1, -1))) % 11
            r = 0 if r == 11 else r
            if r != 10:
                n = body + str(r)
                return f"{n[:3]} {n[3:6]} {n[6:]}"

    def uk_driving(self) -> str:
        m = self.rng.choice(["0", "1", "5", "6"]) + str(self.rng.randint(1, 2))
        return self.u(5) + str(self.rng.randint(0, 9)) + m[0] + str(self.rng.randint(1, 9)) + "1" + str(self.rng.randint(1, 2)) + str(self.rng.randint(0, 9)) + self.u(2) + str(self.rng.randint(0, 9)) + self.u(2)

    def sin(self) -> str:
        body = str(self.rng.choice([1, 2, 3, 4, 5, 6, 7, 9])) + self.d(7)
        n = body + luhn_check(body)
        return f"{n[:3]} {n[3:6]} {n[6:]}"

    def dni(self) -> str:
        n = self.rng.randint(10000000, 99999999)
        return f"{n}{'TRWAGMYFPDXBNJZSQVHLCKE'[n % 23]}"

    def nie(self) -> str:
        lead = self.rng.choice("XYZ")
        body = self.d(7)
        n = int({"X": "0", "Y": "1", "Z": "2"}[lead] + body)
        return f"{lead}{body}{'TRWAGMYFPDXBNJZSQVHLCKE'[n % 23]}"

    def codice_fiscale(self) -> str:
        odd = {**{str(i): v for i, v in enumerate([1, 0, 5, 7, 9, 13, 15, 17, 19, 21])},
               **dict(zip(UP, [1, 0, 5, 7, 9, 13, 15, 17, 19, 21, 2, 4, 18, 20, 11, 3, 6, 8, 12, 14, 16, 10, 22, 25,
                               24, 23]))}
        even = {**{str(i): i for i in range(10)}, **{c: i for i, c in enumerate(UP)}}
        body = (self.u(6) + self.d(2) + self.rng.choice("ABCDEHLMPRST") + f"{self.rng.randint(1, 28):02d}"
                + self.u(1) + self.d(3))
        total = sum(odd[c] if i % 2 == 0 else even[c] for i, c in enumerate(body))
        return body + UP[total % 26]

    def nir(self) -> str:
        body = (self.rng.choice("12") + self.d(2) + f"{self.rng.randint(1, 12):02d}" + f"{self.rng.randint(1, 95):02d}"
                + self.d(3) + self.d(3))
        key = 97 - int(body) % 97
        n = f"{body}{key:02d}"
        return f"{n[0]} {n[1:3]} {n[3:5]} {n[5:7]} {n[7:10]} {n[10:13]} {n[13:]}"

    def personnummer(self) -> str:
        body = self.date6() + self.d(3)
        return f"{body[:6]}-{body[6:]}{luhn_check(body)}"

    def hetu(self) -> str:
        date = self.date6("dmy")
        ind = f"{self.rng.randint(2, 899):03d}"
        return f"{date}-{ind}{'0123456789ABCDEFHJKLMNPRSTUVWXY'[int(date + ind) % 31]}"

    def be_nrn(self) -> str:
        body = self.date6() + self.d(3)
        c = 97 - int(body) % 97
        return f"{body[:2]}.{body[2:4]}.{body[4:6]}-{body[6:9]}.{c:02d}"

    def cpf(self) -> str:
        ds = [int(c) for c in self.d(9)]
        for n in (9, 10):
            r = (sum(d * (n + 1 - i) for i, d in enumerate(ds[:n])) * 10) % 11
            ds.append(0 if r == 10 else r)
        s = "".join(map(str, ds))
        return f"{s[:3]}.{s[3:6]}.{s[6:9]}-{s[9:]}"

    def curp(self) -> str:
        chars = "0123456789ABCDEFGHIJKLMN&OPQRSTUVWXYZ"
        body = (self.u(1) + self.rng.choice("AEIOU") + self.u(2) + self.date6() + self.rng.choice("HM")
                + self.rng.choice(["DF", "JC", "NL", "MC", "GT"]) + self.u(3, "BCDFGHJKLMNPQRSTVWXYZ") + self.d(1))
        total = sum(chars.index(c) * (18 - i) for i, c in enumerate(body))
        return body + str((10 - total % 10) % 10)

    def rfc(self) -> str:
        return self.u(4) + self.date6() + self.u(2) + self.d(1)

    def cuil(self) -> str:
        while True:
            body = self.rng.choice(["20", "23", "27"]) + self.d(8)
            r = 11 - sum(int(c) * w for c, w in zip(body, [5, 4, 3, 2, 7, 6, 5, 4, 3, 2])) % 11
            if r == 10:
                continue
            c = 0 if r == 11 else r
            return f"{body[:2]}-{body[2:]}-{c}"

    def rut(self) -> str:
        n = self.rng.randint(10000000, 25000000)
        total = sum(int(c) * w for c, w in zip(reversed(str(n)), [2, 3, 4, 5, 6, 7] * 2))
        r = 11 - total % 11
        dv = {11: "0", 10: "K"}.get(r, str(r))
        s = str(n)
        return f"{s[:2]}.{s[2:5]}.{s[5:]}-{dv}"

    # existing county-level codes (GB/T 2260) that python-stdnum's cn.ric accepts for any birth year 1950-1999:
    # Beijing Dongcheng and Chaoyang, Shanghai Huangpu, Tianjin Heping (most other districts date from 1983 or later)
    COUNTIES = ["110101", "110105", "310101", "120101"]

    def china_id(self) -> str:
        body = self.rng.choice(self.COUNTIES) + f"19{self.rng.randint(50, 99)}" + \
            f"{self.rng.randint(1, 12):02d}{self.rng.randint(1, 28):02d}" + self.d(3)
        w = [7, 9, 10, 5, 8, 4, 2, 1, 6, 3, 7, 9, 10, 5, 8, 4, 2]
        return body + "10X98765432"[sum(int(c) * x for c, x in zip(body, w)) % 11]

    def kr_rrn(self) -> str:
        # no check digit: numbers issued since October 2020 end in random digits, so the old checksum is not a rule
        return f"{self.date6()}-{self.rng.randint(1, 4)}{self.d(6)}"

    def nric(self) -> str:
        p = self.rng.choice("STFG")
        body = self.d(7)
        s = sum(int(c) * w for c, w in zip(body, [2, 7, 6, 5, 4, 3, 2])) + (4 if p in "TG" else 0)
        table = "JZIHGFEDCBA" if p in "ST" else "XWUTRQPNMLK"
        return p + body + table[s % 11]

    def hkid(self) -> str:
        letter = self.u(1)
        body = self.d(6)
        vals = [36, UP.index(letter) + 10] + [int(c) for c in body]
        s = sum(v * w for v, w in zip(vals, [9, 8, 7, 6, 5, 4, 3, 2]))
        c = (11 - s % 11) % 11
        return f"{letter}{body}({'A' if c == 10 else c})"

    def taiwan(self) -> str:
        m = dict(zip(UP, [10, 11, 12, 13, 14, 15, 16, 17, 34, 18, 19, 20, 21, 22, 35, 23, 24, 25, 26, 27, 28, 29, 32,
                          30, 31, 33]))
        letter = self.u(1)
        body = self.rng.choice("12") + self.d(7)
        n = m[letter]
        s = n // 10 + (n % 10) * 9 + sum(int(c) * w for c, w in zip(body, [8, 7, 6, 5, 4, 3, 2, 1]))
        return letter + body + str((10 - s % 10) % 10)

    def mykad(self) -> str:
        return f"{self.date6()}-{self.rng.randint(1, 16):02d}-{self.d(4)}"

    def cnic(self) -> str:
        return f"{self.rng.randint(1, 7)}{self.d(4)}-{self.d(7)}-{self.d(1)}"

    def lk_nic(self) -> str:
        return f"{self.rng.randint(50, 99)}{self.rng.randint(1, 365):03d}{self.d(4)}V"

    def emirates(self) -> str:
        body = "784" + f"19{self.rng.randint(60, 99)}" + self.d(7)
        n = body + luhn_check(body)
        return f"{n[:3]}-{n[3:7]}-{n[7:14]}-{n[14]}"

    def za_id(self) -> str:
        body = self.date6() + self.d(4) + "0" + "8"
        return body + luhn_check(body)

    def snils(self) -> str:
        while True:
            body = self.d(9)
            s = sum(int(c) * w for c, w in zip(body, range(9, 0, -1)))
            if s < 100:
                c = s
            elif s in (100, 101):
                c = 0
            else:
                c = s % 101
                c = 0 if c == 100 else c
            return f"{body[:3]}-{body[3:6]}-{body[6:]} {c:02d}"

    def inn(self) -> str:
        body = [int(c) for c in self.d(10)]
        c1 = sum(d * w for d, w in zip(body, [7, 2, 4, 10, 3, 5, 9, 4, 6, 8])) % 11 % 10
        body.append(c1)
        c2 = sum(d * w for d, w in zip(body, [3, 7, 2, 4, 10, 3, 5, 9, 4, 6, 8])) % 11 % 10
        body.append(c2)
        return "".join(map(str, body))

    def iban(self, country: str = "DE", bban_len: int = 18, bank_letters: int = 0) -> str:
        # bank_letters: a BBAN that starts with the bank's letters (GB: four, then the sort code and account)
        bban = self.u(bank_letters) + self.d(bban_len - bank_letters)
        num = "".join(str(int(c, 36)) for c in bban + country + "00")
        check = 98 - int(num) % 97
        s = f"{country}{check:02d}{bban}"
        return " ".join(s[i:i + 4] for i in range(0, len(s), 4))

    def steuer_id(self) -> str:
        while True:
            body = str(self.rng.randint(1, 9)) + self.d(9)
            # exactly one digit appears two or three times, every other at most once (python-stdnum's de.idnr)
            repeated = [body.count(c) for c in set(body) if body.count(c) > 1]
            if len(repeated) != 1 or repeated[0] not in (2, 3):
                continue
            product = 10
            for c in body:
                s = (int(c) + product) % 10 or 10
                product = (s * 2) % 11
            return body + str((11 - product) % 10)

    def ausweis(self) -> str:
        alphabet = "0123456789CFGHJKLMNPRTVWXYZ"
        body = self.u(1, "CFGHJKLMNPRTVWXYZ") + "".join(self.rng.choice(alphabet) for _ in range(7)) + self.d(1)
        s = sum(int(c, 36) * [7, 3, 1][i % 3] for i, c in enumerate(body))
        return body + str(s % 10)

    def bsn(self) -> str:
        while True:
            body = self.d(8)
            s = sum(int(c) * w for c, w in zip(body, range(9, 1, -1)))
            last = s % 11
            if last <= 9 and s > 0:
                return body + str(last)

    def pesel(self) -> str:
        body = self.date6() + self.d(4)
        s = sum(int(c) * w for c, w in zip(body, [1, 3, 7, 9, 1, 3, 7, 9, 1, 3]))
        return body + str((10 - s % 10) % 10)

    def cpr(self) -> str:
        return f"{self.date6('dmy')}-{self.d(4)}"

    def pt_nif(self) -> str:
        while True:
            body = self.rng.choice("123456789") + self.d(7)
            r = 11 - sum(int(c) * w for c, w in zip(body, range(9, 1, -1))) % 11
            return body + str(0 if r >= 10 else r)

    def ppsn(self) -> str:
        letters = "WABCDEFGHIJKLMNOPQRSTUV"
        body = self.d(7)
        s = sum(int(c) * w for c, w in zip(body, range(8, 1, -1)))
        return body + letters[s % 23]

    def tfn(self) -> str:
        w = [1, 4, 3, 7, 5, 8, 6, 9, 10]
        while True:
            n = self.d(9)
            if sum(int(c) * x for c, x in zip(n, w)) % 11 == 0:
                return f"{n[:3]} {n[3:6]} {n[6:]}"

    def au_medicare(self) -> str:
        body = str(self.rng.randint(2, 6)) + self.d(7)
        c = sum(int(c) * w for c, w in zip(body, [1, 3, 7, 9, 1, 3, 7, 9])) % 10
        return body + str(c) + self.d(1)

    def my_number(self) -> str:
        body = self.d(11)
        s = sum(int(body[11 - n]) * (n + 1 if n <= 6 else n - 5) for n in range(1, 12))
        r = s % 11
        return body + str(0 if r <= 1 else 11 - r)

    # existing province codes (Aceh, North Sumatra, Jakarta, West Java, Central Java, East Java, Bali, South Sulawesi)
    PROVINCES = ["11", "12", "31", "32", "33", "35", "51", "73"]

    def nik(self) -> str:
        return self.rng.choice(self.PROVINCES) + self.d(4) + self.date6("dmy") + self.d(4)

    def tckn(self) -> str:
        ds = [self.rng.randint(1, 9)] + [int(c) for c in self.d(8)]
        d10 = ((ds[0] + ds[2] + ds[4] + ds[6] + ds[8]) * 7 - (ds[1] + ds[3] + ds[5] + ds[7])) % 10
        ds.append(d10)
        ds.append(sum(ds) % 10)
        return "".join(map(str, ds))

    def il_id(self) -> str:
        body = self.d(8)
        s = 0
        for i, c in enumerate(body):
            x = int(c) * (1 if i % 2 == 0 else 2)
            s += x // 10 + x % 10
        return body + str((10 - s % 10) % 10)

    def saudi(self) -> str:
        body = self.rng.choice("12") + self.d(8)
        return body + luhn_check(body)

    def kra(self) -> str:
        return self.rng.choice("AP") + self.d(9) + self.u(1)

    def il_id_short(self) -> str:
        """Seven digits and the check digit, written 0000000-0: the nine-digit number with a leading zero."""
        n = self.il_id_from("0" + self.d(7))
        return f"{n[1:8]}-{n[8]}"

    def il_id_from(self, body: str) -> str:
        s = 0
        for i, c in enumerate(body):
            x = int(c) * (1 if i % 2 == 0 else 2)
            s += x // 10 + x % 10
        return body + str((10 - s % 10) % 10)

    def sg_fin_m(self) -> str:
        """Singapore's M-series FIN: its check-letter table is unpublished, so the letter is any (shape only)."""
        return "M" + self.d(7) + self.u(1)

    def taiwan_arc(self) -> str:
        """Taiwan's resident certificate number: 8 or 9 as the second character, the national id's check."""
        m = dict(zip(UP, [10, 11, 12, 13, 14, 15, 16, 17, 34, 18, 19, 20, 21, 22, 35, 23, 24, 25, 26, 27, 28, 29, 32,
                          30, 31, 33]))
        letter = self.u(1)
        body = self.rng.choice("89") + self.d(7)
        n = m[letter]
        s = n // 10 + (n % 10) * 9 + sum(int(c) * w for c, w in zip(body, [8, 7, 6, 5, 4, 3, 2, 1]))
        return letter + body + str((10 - s % 10) % 10)

    def lk_nic_new(self) -> str:
        """Sri Lanka's 12-digit NIC: year, day of the year (+500 for women), serial, a check digit of no published
        rule (any)."""
        day = self.rng.randint(1, 365) + self.rng.choice([0, 500])
        return f"{self.rng.randint(1950, 2005)}{day:03d}{self.d(5)}"

    def ausweis_front(self) -> str:
        """The nine characters the Personalausweis prints on its front (the check digit is in the MRZ only)."""
        return self.ausweis()[:9]

    def ppsn_old_t(self) -> str:
        """An old-format PPS number with the ninth letter T, which the check ignores."""
        return self.ppsn() + "T"

    def no_fnr(self) -> str:
        """Norway's fødselsnummer: DDMMYY, individual digits 000-499 (born 1900-1999), two mod-11 check digits."""
        while True:
            body = self.date6("dmy") + f"{self.rng.randint(0, 499):03d}"
            c1 = (11 - sum(int(c) * w for c, w in zip(body, [3, 7, 6, 1, 8, 9, 4, 5, 2]))) % 11
            if c1 == 10:
                continue
            body += str(c1)
            c2 = (11 - sum(int(c) * w for c, w in zip(body, [5, 4, 3, 2, 7, 6, 5, 4, 3, 2]))) % 11
            if c2 == 10:
                continue
            return body + str(c2)

    def clabe(self) -> str:
        """Mexico's CLABE: bank (3), branch (3), account (11), check digit (weights 3 7 1, mod 10)."""
        body = self.d(17)
        s = sum(int(c) * w % 10 for c, w in zip(body, [3, 7, 1] * 6))
        return body + str((10 - s % 10) % 10)
